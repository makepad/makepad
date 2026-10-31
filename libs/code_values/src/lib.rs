//! Inline value controls over Splash in Makepad's code editor: the code is
//! the editor, and each value carries a small control where it is written.
//!
//! - a **swatch** before a colour (`#ff5a36`): a click opens a colour wheel;
//! - a **curve** before an ease (`@ease_out_back`): a click opens the eases,
//!   each drawn as its curve;
//! - a **chip** after a name (`@center`): a click opens its choices;
//! - a **pencil** before words (`"Make it move"`): a click opens a words box
//!   (real line breaks, no escapes), live as it is typed;
//! - every **number** scrubs where it is written: alt-drag on it, up or right
//!   (a step per point at its own precision; shift for finer).
//!
//! The host says what each value is: its document's
//! [`makepad_edit_values::ValueSite`]s (kind, unit, range, choices and where
//! it is written); the control is picked from the kind and the literal as
//! written. The controls reserve their columns in the text
//! ([`CodeDocument::set_inline_inlays`]: the code moves aside, nothing is
//! drawn over it) and answer with [`Edit`]s whose splices replace exactly
//! the value's bytes, one gesture id per gesture (the host folds a
//! gesture's edits into one undo step) with `done` on its last.
//!
//! The host is a widget that wraps a `CodeView` (Motion's Source view, the
//! Score editor): it holds a [`CodeValues`], gives it every event first
//! ([`CodeValues::handle_event`]), lays it out before the view draws
//! ([`CodeValues::sync`]) and draws it after ([`CodeValues::draw`]).

mod data;
mod cache;
mod chips;
mod gizmo;
mod grid;
mod pad;
mod plot;

use data::{control_of_literal, lit_at};
pub use makepad_edit_values::{Change, Edit, Splice, ValueKind, ValueSite};

use cache::Caches;
use chips::Summary;
use plot::{PlotDrag, PLOT_H, PLOT_W};
use std::rc::Rc;

use makepad_code_editor::code_view::CodeView;
use makepad_code_editor::CodeDocument;
use makepad_code_editor::inlays::InlineInlay;
use makepad_code_editor::text::Position;
use makepad_code_editor::widgets::InlineWidget;
use makepad_widgets::fab_controls::{ColorWheelAction, FabColorWheel};
use makepad_widgets::*;
use std::ops::Range;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawCodeValue::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y
            if self.shape > 4.5 {
                // A line from fill.xy to fill.zw (local points), stroke wide.
                sdf.move_to(self.fill.x, self.fill.y)
                sdf.line_to(self.fill.z, self.fill.w)
                sdf.stroke(self.edge, self.stroke)
                return sdf.result
            }
            if self.shape > 3.5 {
                // A direction: an arrow from a sphere's middle (fill.xy
                // across and up, fill.z toward you: a tip behind is dimmer).
                let c = vec2(w * 0.5, h * 0.5)
                let r = min(w, h) * 0.5 - 1.0
                sdf.circle(c.x, c.y, r)
                sdf.fill_keep(vec4(1.0, 1.0, 1.0, 0.06))
                sdf.stroke(vec4(self.edge.xyz, 0.45), 1.0)
                let tip = c + vec2(self.fill.x, 0.0 - self.fill.y) * (r - 1.0)
                sdf.move_to(c.x, c.y)
                sdf.line_to(tip.x, tip.y)
                sdf.stroke(self.edge, 1.4)
                sdf.circle(tip.x, tip.y, 2.1)
                sdf.fill(vec4(self.edge.xyz, mix(0.45, 1.0, step(0.0, self.fill.z))))
                return sdf.result
            }
            if self.shape > 2.5 {
                // A chevron pointing down (a choice's chip).
                let p = self.pos * self.rect_size
                let c = vec2(w * 0.5, h * 0.5)
                let d = abs(p.x - c.x) - (c.y + 2.0 - p.y)
                let ink = (1.0 - smoothstep(-0.6, 0.6, abs(d) - 0.7)) * step(abs(p.x - c.x), 3.5) * step(abs(p.y - c.y), 3.0)
                return vec4(self.edge.xyz * self.edge.w * ink, self.edge.w * ink)
            }
            if self.shape > 1.5 {
                // A swatch: the colour over a checker where it is clear.
                let q = floor(self.pos * self.rect_size / 4.0)
                let odd = fract((q.x + q.y) * 0.5) * 2.0
                let checker = mix(vec4(0.55, 0.55, 0.55, 1.0), vec4(0.8, 0.8, 0.8, 1.0), odd)
                let fill = mix(checker, vec4(self.fill.xyz, 1.0), self.fill.w)
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, self.radius)
                sdf.fill_keep(fill)
                sdf.stroke(self.edge, self.stroke)
                return sdf.result
            }
            if self.shape > 0.5 {
                // A solid rectangle: a curve's column, an underline.
                return vec4(self.fill.xyz * self.fill.w, self.fill.w)
            }
            sdf.box(0.5, 0.5, w - 1.0, h - 1.0, self.radius)
            sdf.fill_keep(self.fill)
            sdf.stroke(self.edge, self.stroke)
            return sdf.result
        }
    }

    set_type_default() do #(DrawCodeValueTex::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex: texture_2d(float)
        pixel: fn() {
            return self.tex.sample_nearest(self.pos)
        }
    }

    mod.widgets.CodeValues = set_type_default() do #(CodeValues::script_component(vm)){
        draw_label +: {color: #c9d1d9 text_style: theme.font_regular{font_size: 8.5}}
        draw_glyph +: {color: #c9d1d9 text_style: theme.font_regular{font_size: 7.5}}
        wheel: mod.widgets.FabColorWheel{width: 168 height: 168}
        words: TextInput{width: 320 height: 80 margin: 0 padding: Inset{left: 8 right: 8 top: 6 bottom: 6} empty_text: ""
            is_multiline: true
            draw_bg +: {color: #0d1117 color_hover: #0d1117 color_focus: #0d1117 border_color: #ff7a2f border_size: 1.0}
            draw_text +: {text_style: theme.font_regular{font_size: 10}}}
    }
}

/// The control a value shows: picked from its site's kind and its literal
/// as written (the payloads are read from the source).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Control {
    /// A colour, 0..1 RGBA: a swatch before it, a colour wheel.
    Color([f32; 4]),
    /// An ease, as its curve sampled over 0..1: a curve before it; the
    /// choices are the eases (each with its curve).
    Ease { curve: Vec<f32>, choices: Vec<Choice> },
    /// A name from a set: a chip after it, a menu.
    Choice { choices: Vec<Choice> },
    /// Words in a string literal: a pencil before it, a words box.
    Words { text: String },
    /// A number: scrubbed where it is written (alt-drag), dragged as a chip.
    Number,
    /// A list of numbers: a plot before it; its items dragged in a plot,
    /// as values over their index or (`pairs`) as x, y points (a shape),
    /// or typed in a grid. `name`: what it is written under (the popup's
    /// title); `axis`: what its index means.
    Data { values: Vec<f32>, pairs: bool, name: String, axis: DataAxis },
    /// A list of colours `[#hex, …]`: a strip of swatches; a click on one
    /// opens the colour wheel for it.
    Swatches { colors: Vec<[f32; 4]> },
    /// A list of records (lists or objects of numbers and words): `rows` ×
    /// `cols`, the columns' `heads` (the objects' keys, else their index);
    /// a grid of typed cells. Its cells are read from the source
    /// ([`data::table_of`]).
    Table { rows: usize, cols: usize, heads: Vec<String> },
    /// A 2D point (`vec2(x, y)`): a dot on a small pad; dragged on a pad.
    Point([f32; 2]),
    /// A 3D point (`vec3(x, y, z)`): its three numbers; dragged in a small
    /// 3D view.
    Point3([f32; 3]),
    /// A direction (`vec3`, a light's `dir`): an arrow in a sphere; turned
    /// on a sphere (its written length kept).
    Direction([f32; 3]),
    /// 3D keys (`[vec3(..), …]`, a camera's path): the path in a small 3D
    /// view, its keys dragged.
    Path3 { keys: Vec<[f32; 3]> },
}

/// What a number list's index means (the plot's x axis).
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) enum DataAxis {
    /// Item 0, 1, 2…
    #[default]
    Index,
    /// Samples over time: item `k` is at `start + k / per_second` seconds.
    Time { per_second: f64, start: f64 },
}

impl DataAxis {
    /// The x axis's name.
    pub fn label(&self) -> &'static str {
        match self {
            DataAxis::Index => "index",
            DataAxis::Time { .. } => "seconds",
        }
    }
}

/// One entry of a menu: what it says, the source text it writes, and a
/// curve to draw beside it (eases).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Choice {
    pub label: String,
    pub text: String,
    pub curve: Option<Vec<f32>>,
}

/// A value placed in the source: its bytes, its control and the limits a
/// typed site gives it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Site {
    pub range: Range<usize>,
    pub control: Control,
    pub limits: makepad_edit_values::Range,
}

/// What [`CodeValues::handle_event`] did.
#[derive(Default)]
pub struct CodeValuesResponse {
    /// The event was the controls' (the editor must not see it).
    pub consumed: bool,
    pub redraw: bool,
    /// Splices of file 0 (the source the sites came from).
    pub edits: Vec<Edit>,
    /// A value was picked (its control pressed or a menu opened): the host
    /// may select it in its other views. The source byte it starts at.
    pub focus: Option<usize>,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCodeValue {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    fill: Vec4f,
    #[live]
    edge: Vec4f,
    #[live]
    stroke: f32,
    #[live]
    radius: f32,
    /// 0 box, 1 solid, 2 swatch, 3 chevron, 4 sphere arrow, 5 line.
    #[live]
    shape: f32,
}

/// A 3D view's picture, as its pass drew it.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawCodeValueTex {
    #[deref]
    draw_super: DrawQuad,
}

/// Columns a control takes in the text.
const SWATCH_COLS: usize = 2;
const CURVE_COLS: usize = 3;
const CHIP_COLS: usize = 2;
const PENCIL_COLS: usize = 2;
const PLOT_COLS: usize = 8;
/// Points of pointer travel per number step.
const SCRUB_PX: f64 = 1.0;
const MENU_ROW: f64 = 22.0;
const MENU_W: f64 = 220.0;
const MENU_MAX_ROWS: usize = 14;

/// The strip of a list's colours under the wheel.
const WHEEL_STRIP: f64 = 26.0;

fn wheel_strip(rect: Rect) -> Rect {
    Rect { pos: dvec2(rect.pos.x + 8.0, rect.pos.y + rect.size.y - WHEEL_STRIP), size: dvec2(rect.size.x - 16.0, 18.0) }
}

/// An axis bound, short.
pub(crate) fn fmt_axis(v: f64) -> String {
    let text = format!("{v:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Where a control sits relative to its value.
fn placement(control: &Control) -> Option<(usize, bool)> {
    Some(match control {
        Control::Color(_) => (SWATCH_COLS, true),
        Control::Ease { .. } => (CURVE_COLS, true),
        Control::Choice { .. } => (CHIP_COLS, false),
        Control::Words { .. } => (PENCIL_COLS, true),
        Control::Data { .. } => (PLOT_COLS, true),
        Control::Swatches { colors } => ((colors.len() + 2).clamp(4, 10), true),
        Control::Table { .. } => (6, true),
        Control::Point(_) | Control::Direction(_) => (2, true),
        Control::Point3(_) => (3, true),
        Control::Path3 { .. } => (4, true),
        Control::Number => return None,
    })
}

/// The open popup.
#[derive(Clone, Debug)]
enum Popup {
    /// `item`: a swatch of a list of colours (its strip shows under the
    /// wheel).
    Wheel { site: usize, rect: Rect, alpha: f32, item: Option<usize> },
    /// `armed`: the press that opened it is over (its release chooses
    /// nothing; a later press or a drag away from it does).
    Menu { site: usize, rect: Rect, hover: Option<usize>, top: usize, armed: bool, from: DVec2 },
    Words { site: usize, rect: Rect, focus: bool },
    /// A list's items as a plot: the point under the pointer, the one
    /// being dragged, and whether items pair up as x, y points.
    Plot { site: usize, rect: Rect, hover: Option<usize>, drag: Option<PlotDrag>, pairs: bool },
    /// A list's items or a table's records as typed cells.
    Grid { site: usize, rect: Rect, grid: grid::GridState },
    /// A 2D point on a pad.
    Pad { site: usize, rect: Rect, pad: pad::PadState },
    /// A 3D point, direction or path in a 3D view.
    Space { site: usize, rect: Rect, space: Box<gizmo::SpaceState> },
}

impl Popup {
    fn set_site(&mut self, to: usize) {
        match self {
            Popup::Wheel { site, .. } | Popup::Menu { site, .. } | Popup::Words { site, .. } | Popup::Plot { site, .. } | Popup::Grid { site, .. } | Popup::Pad { site, .. } | Popup::Space { site, .. } => *site = to,
        }
    }

    fn site(&self) -> usize {
        match self {
            Popup::Wheel { site, .. } | Popup::Menu { site, .. } | Popup::Words { site, .. } | Popup::Plot { site, .. } | Popup::Grid { site, .. } | Popup::Pad { site, .. } | Popup::Space { site, .. } => *site,
        }
    }
}

/// The numbers of the list `[…]` starting at byte `start` of `source`: each
/// one's bytes and value.
pub fn data_items(source: &str, start: usize) -> Vec<(Range<usize>, f64)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    if bytes.get(start) != Some(&b'[') {
        return out;
    }
    let mut i = start + 1;
    while i < bytes.len() && bytes[i] != b']' {
        let b = bytes[i];
        if b.is_ascii_digit() || b == b'.' || (b == b'-' && bytes.get(i + 1).is_some_and(|c| c.is_ascii_digit() || *c == b'.')) {
            let from = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.' || bytes[i] == b'e' || (bytes[i] == b'-' && bytes[i - 1] == b'e')) {
                i += 1;
            }
            if let Ok(v) = source[from..i].parse::<f64>() {
                out.push((from..i, v));
            }
        } else if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// A list's value range to plot in, a little wider than its values.
pub(crate) fn plot_range(values: impl Iterator<Item = f64>) -> (f64, f64) {
    let (lo, hi) = values.fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    if lo > hi {
        return (0.0, 1.0);
    }
    let pad = ((hi - lo) * 0.15).max(1e-3).max(if hi == lo { hi.abs() * 0.5 + 0.5 } else { 0.0 });
    (lo - pad, hi + pad)
}

/// A dragged item's value as written: the places it was written with (at
/// least enough for the plot's span).
pub(crate) fn fmt_item(value: f64, written: &str, span: f64) -> String {
    let written_places = written.split('.').nth(1).map_or(0, str::len);
    let needed = if span <= 0.0 { 0 } else { (2.0 - span.log10().floor()).clamp(0.0, 6.0) as usize };
    let places = written_places.max(needed.min(written_places.max(3)));
    let text = format!("{value:.places$}");
    if text.starts_with("-0") && text.trim_start_matches(['-', '0', '.']).is_empty() { text[1..].to_string() } else { text }
}

/// A gesture writing one value: its id and the value's current bytes.
#[derive(Clone, Debug)]
struct Gesture {
    id: u64,
    span: Range<usize>,
    last: String,
    /// The value as written when the gesture began (Escape returns to it).
    original: String,
    /// A number scrubbed: where the drag began, the value then and its
    /// decimals.
    scrub: Option<(DVec2, f64, usize)>,
}

#[derive(Script, ScriptHook)]
pub struct CodeValues {
    #[live]
    draw_ctl: DrawCodeValue,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_glyph: DrawText,
    #[live]
    pop_list: DrawList2d,
    #[live]
    wheel: FabColorWheel,
    #[live]
    words: TextInput,
    /// The source the sites were read from.
    #[rust]
    source: String,
    #[rust]
    sites: Vec<Site>,
    /// The inlays as last set, the document and text they were set on (a
    /// new document, or text edited since, is laid out again).
    #[rust]
    inlays: Vec<(usize, usize, usize)>,
    #[rust]
    laid_on: Option<(CodeDocument, String)>,
    /// Control rects drawn this frame: (rect, site).
    #[rust]
    hots: Vec<(Rect, usize)>,
    #[rust]
    hover: Option<usize>,
    #[rust]
    popup: Option<Popup>,
    /// The words editor has seen the press that opened it end: until then a
    /// lost focus (the release landing elsewhere) is taken back.
    #[rust]
    words_settled: bool,
    #[rust]
    gesture: Option<Gesture>,
    #[rust]
    gestures: u64,
    /// Whether the editor's text is the source the sites were read from
    /// (while someone types, the controls rest).
    #[rust]
    fresh: bool,
    #[rust]
    clip: Rect,
    /// The alt key is down over a number (the scrub cursor).
    #[rust]
    alt_number: Option<Range<usize>>,
    /// The value a click on the picture landed on.
    #[rust]
    focus: Option<usize>,
    /// The sites' revision and where the source's lines start; what the
    /// layout was last made for (sites).
    #[rust]
    sites_rev: u64,
    #[rust]
    line_starts: Vec<usize>,
    #[rust]
    synced: Option<u64>,
    /// What each site's chip draws (made when the sites change).
    #[rust]
    summaries: Vec<Summary>,
    /// Long literals as read (once per change of the source).
    #[rust]
    caches: Caches,
    /// Swatches drawn this frame: (rect, site, item).
    #[rust]
    strip_hits: Vec<(Rect, usize, usize)>,
    /// The 3D views' pass, picture and line renderer; what was drawn last.
    #[live]
    draw_tex: DrawCodeValueTex,
    #[new]
    space_pass: DrawPass,
    #[new]
    space_list: DrawList,
    #[new]
    space_color: Texture,
    #[new]
    space_depth: Texture,
    #[rust]
    space_ready: bool,
    #[rust]
    lines: Option<makepad_render_lines::LineRenderer>,
    #[rust]
    space_drawn: Option<String>,
    /// How many times a 3D view's picture was drawn (it is drawn only when
    /// what it shows changes).
    #[rust]
    space_renders: u64,
    /// A key whose press a popup took: its release is taken too.
    #[rust]
    swallow_up: Option<KeyCode>,
}

/// A typed site's control where it is written in `source` (file 0): from
/// its kind, and from its literal for points, lists and tables.
fn place(source: &str, site: &ValueSite) -> Option<Site> {
    let (file, range) = site.span.clone()?;
    if file != 0 {
        return None;
    }
    let text = source.get(range.clone())?;
    let key = data::key_before(source, range.start);
    let control = match site.kind {
        ValueKind::Colour => match lit_at(text, 0)? {
            data::Lit::Color { rgba, .. } => Control::Color(rgba),
            _ => control_of_literal(text, &key)?,
        },
        ValueKind::Ease => Control::Ease { curve: ease_curve(text), choices: ease_choices(site, text) },
        ValueKind::Choice => Control::Choice { choices: site.choices.iter().map(|c| choice(text, c)).collect() },
        ValueKind::Bool => Control::Choice { choices: ["false", "true"].iter().map(|c| choice(text, c)).collect() },
        ValueKind::Text => match lit_at(text, 0)? {
            data::Lit::Str { text, .. } => Control::Words { text },
            _ => return None,
        },
        // A list of a time's samples: `step` seconds apart.
        ValueKind::Time if site.step > 0.0 => match control_of_literal(text, &key) {
            Some(Control::Data { values, pairs, name, .. }) => Control::Data { values, pairs, name, axis: DataAxis::Time { per_second: 1.0 / site.step, start: 0.0 } },
            other => other.unwrap_or(Control::Number),
        },
        kind if kind.is_numeric() => control_of_literal(text, &key).unwrap_or(Control::Number),
        _ => control_of_literal(text, &key)?,
    };
    Some(Site { range, control, limits: site.range })
}

/// A menu entry written the way the value is written now (`@saw`, `"saw"` or `saw`).
fn choice(current: &str, label: &str) -> Choice {
    let current = current.trim();
    let bare = label.trim_start_matches('@');
    let text = if current.starts_with('@') {
        format!("@{bare}")
    } else if current.starts_with('"') {
        quote(bare)
    } else {
        bare.to_string()
    };
    Choice { label: bare.to_string(), text, curve: None }
}

/// An ease's curve over 0..1 (linear for one that is not a name).
fn ease_curve(written: &str) -> Vec<f32> {
    let ease = makepad_tween::named::Ease::from_id(written.trim().trim_start_matches('@')).unwrap_or_default();
    (0..=24).map(|i| ease.apply(i as f64 / 24.0, 1.0) as f32).collect()
}

/// The eases a site offers (every named ease when it names none), each with its curve.
fn ease_choices(site: &ValueSite, written: &str) -> Vec<Choice> {
    let names = if site.choices.is_empty() { makepad_tween::named::Ease::ids() } else { site.choices.clone() };
    names
        .iter()
        .map(|name| {
            let mut c = choice(written, name);
            if !c.text.starts_with('@') && !c.text.starts_with('"') {
                c.text = format!("@{}", c.label);
            }
            c.curve = Some(ease_curve(&c.text));
            c
        })
        .collect()
}

/// `text` (a number scrubbed at `at`) within the hard limits of the typed
/// site written there, at the precision written.
fn within(sites: &[Site], at: usize, text: String, decimals: usize) -> String {
    let Some(site) = sites.iter().find(|s| s.range.start == at && s.control == Control::Number) else { return text };
    let Ok(value) = text.parse::<f64>() else { return text };
    let clamped = site.limits.clamp(value);
    if clamped == value { text } else { format!("{clamped:.decimals$}") }
}

/// A Splash string literal for `text`.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A colour as Splash writes it: `#rrggbb`, or `#rrggbbaa` when not opaque.
pub fn fmt_color(rgba: [f32; 4]) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let [r, g, b, a] = rgba.map(byte);
    if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

fn rgb_to_hsv(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max <= 0.0 { 0.0 } else { d / max };
    [h, s, max]
}

/// The number literal holding byte `at` in Splash `source` (not inside a
/// string, a comment, a name or a colour), with a leading minus that is a
/// sign rather than an operator.
pub fn number_at(source: &str, at: usize) -> Option<Range<usize>> {
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b'#' | b'@' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
            }
            _ if b.is_ascii_alphabetic() || b == b'_' => {
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
            }
            _ if b.is_ascii_digit() || (b == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) => {
                let mut start = i;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    i += 1;
                }
                // `-3` after an opening, a separator or an operator is a sign.
                if start > 0 && bytes[start - 1] == b'-' {
                    let before = source[..start - 1].trim_end().as_bytes().last().copied();
                    if before.is_none_or(|c| matches!(c, b'(' | b'[' | b'{' | b',' | b':' | b'=' | b'+' | b'-' | b'*' | b'/' | b'\n')) || source[..start - 1].ends_with(char::is_whitespace) && before.is_some_and(|c| c == b':' || c == b',') {
                        start -= 1;
                    }
                }
                if start <= at && at <= i {
                    return Some(start..i);
                }
                if start > at {
                    return None;
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// A number as scrubbed: `base` moved by `steps` at `decimals` places
/// (numbers of 100 and more move by ones).
pub fn scrubbed(base: f64, decimals: usize, steps: f64) -> String {
    let step = 10f64.powi(-(decimals as i32)).max(if base.abs() >= 100.0 { 1.0 } else { 0.0 });
    let value = base + steps.round() * step;
    let text = format!("{value:.decimals$}");
    if text.starts_with("-0") && text.trim_start_matches(['-', '0', '.']).is_empty() { text[1..].to_string() } else { text }
}

impl CodeValues {
    /// The values of `source` the host knows. Call when the source or what
    /// the host knows of it changes; [`Self::sync`] lays the controls out.
    ///
    /// `sites`: the document's typed sites (their spans in file 0); the ones
    /// written in `source` get a control picked from their kind and literal.
    /// Every number scrubs, typed or not.
    pub fn set_sites(&mut self, source: &str, typed: &[ValueSite]) {
        let mut sites: Vec<Site> = typed.iter().filter_map(|site| place(source, site)).collect();
        sites.sort_by_key(|site| (site.range.start, site.range.end));
        sites.dedup_by(|a, b| a.range == b.range);
        sites.retain(|site| site.range.end <= source.len() && source.is_char_boundary(site.range.start) && source.is_char_boundary(site.range.end));
        // The open popup's value stays while its popup is open, even when
        // the host's new sites leave it out (a press selects its region and
        // the shown rows change under it): its text unchanged, it is kept.
        if let Some(open) = self.popup.as_ref().and_then(|p| self.sites.get(p.site())) {
            let same_text = source.get(open.range.clone()).is_some() && source.get(open.range.clone()) == self.source.get(open.range.clone());
            if same_text && !sites.iter().any(|s| s.range.start == open.range.start) {
                let at = sites.partition_point(|s| (s.range.start, s.range.end) < (open.range.start, open.range.end));
                sites.insert(at, open.clone());
            }
        }
        if self.source == source && self.sites == sites {
            return;
        }
        if self.source != source {
            self.caches.changed();
        }
        // The open popup and the hovered chip follow their values (the
        // host may give the sites again in another order, or more or fewer).
        let popup_at = self.popup.as_ref().and_then(|p| self.sites.get(p.site())).map(|s| s.range.start);
        let hover_at = self.hover.and_then(|h| self.sites.get(h)).map(|s| s.range.start);
        self.source = source.to_string();
        self.summaries = sites.iter().map(|site| chips::summarize(source, site)).collect();
        self.sites = sites;
        self.sites_rev += 1;
        self.line_starts = std::iter::once(0).chain(source.match_indices('\n').map(|(i, _)| i + 1)).collect();
        if let Some(gesture) = &self.gesture {
            if gesture.scrub.is_none() && self.popup.is_none() {
                self.gesture = None;
            }
        }
        // A popup on a value that is gone closes.
        if let Some(at) = popup_at {
            match self.site_at(at) {
                Some(index) => {
                    if let Some(p) = self.popup.as_mut() {
                        p.set_site(index);
                    }
                }
                None => self.popup = None,
            }
        }
        self.hover = hover_at.and_then(|at| self.site_at(at));
    }

    /// Reserve the controls' columns in `view`'s document (when its text is
    /// the source the sites came from) and relayout when they moved.
    pub fn sync(&mut self, view: &mut CodeView) {
        let Some(session) = view.session.as_ref() else { return };
        let text = session.document().as_text().to_string();
        self.fresh = text == self.source;
        if !self.fresh {
            return;
        }
        // Laid out for these sites, at this width, on this document: done.
        let same_document = self.laid_on.as_ref().is_some_and(|(document, laid)| document.is_same(session.document()) && *laid == text);
        if same_document && self.synced == Some(self.sites_rev) {
            return;
        }
        self.synced = Some(self.sites_rev);
        let inlays: Vec<(usize, usize, usize)> = self.sites.iter().filter_map(|site| {
            let (cols, before) = placement(&site.control)?;
            let (line, byte) = self.line_byte(if before { site.range.start } else { site.range.end });
            Some((line, byte, cols))
        }).collect();
        if inlays == self.inlays && same_document {
            return;
        }
        self.inlays = inlays.clone();
        self.laid_on = Some((session.document().clone(), text));
        session.document().set_inline_inlays(inlays.into_iter().map(|(line, byte, cols)| (line, byte, InlineInlay::Widget(InlineWidget { column_count: cols }))).collect());
        session.relayout_inline();
    }

    /// `(line, byte in line)` of byte `at` of the source.
    fn line_byte(&self, at: usize) -> (usize, usize) {
        let line = self.line_starts.partition_point(|&s| s <= at).saturating_sub(1);
        (line, at - self.line_starts.get(line).copied().unwrap_or(0))
    }

    /// The rect a site's control occupies on screen.
    fn control_rect(&self, view: &CodeView, origin: DVec2, site: &Site, column: f64, line_h: f64) -> Option<Rect> {
        let session = view.session.as_ref()?;
        let (cols, before) = placement(&site.control)?;
        let at = if before { site.range.start } else { site.range.end };
        let (line, byte) = self.line_byte(at);
        let cell = view.editor.position_rect(session, Position { line_index: line, byte_index: byte })?;
        let w = cols as f64 * column;
        let x = origin.x + cell.pos.x - w;
        Some(Rect { pos: dvec2(x + 1.0, origin.y + cell.pos.y + 2.0), size: dvec2(w - 3.0, line_h - 4.0) })
    }

    /// Draw the controls over `view` (call after the view drew), inside
    /// `clip` (the editor's text area), then any popup above everything.
    pub fn draw(&mut self, cx: &mut Cx2d, scope: &mut Scope, view: &CodeView, clip: Rect) {
        self.hots.clear();
        self.strip_hits.clear();
        self.clip = clip;
        let Some(session) = view.session.as_ref() else { return };
        let origin = view.editor.area().rect(cx).pos;
        let metrics = view.editor.metrics(session);
        let column = metrics.column_advance.max(1.0);
        let line_h = metrics.line_advance.max(1.0);
        if self.fresh {
            cx.begin_turtle(Walk::abs_rect(clip), Layout::default().with_clip(true, true));
            // Only the lines in view, and not a folded one (drawn small, its
            // controls would pile up on it).
            let lines = session.layout().view_line_range();
            let from = self.line_starts.get(lines.start).copied().unwrap_or(usize::MAX);
            let to = self.line_starts.get(lines.end).copied().unwrap_or(self.source.len() + 1);
            let first = self.sites.partition_point(|s| s.range.end < from);
            for index in first..self.sites.len() {
                if self.sites[index].range.start > to {
                    break;
                }
                let (line, _) = self.line_byte(self.sites[index].range.start);
                if session.is_folded(line) {
                    continue;
                }
                let site = self.sites[index].clone();
                let Some(rect) = self.control_rect(view, origin, &site, column, line_h) else { continue };
                if rect.pos.y + rect.size.y < clip.pos.y || rect.pos.y > clip.pos.y + clip.size.y {
                    continue;
                }
                let hot = self.hover == Some(index) || self.popup_site() == Some(index);
                if !self.draw_value_inline(cx, index, rect, hot) {
                    self.draw_control(cx, rect, &site.control, hot);
                }
                self.hots.push((rect, index));
            }
            // The number under an alt-hover: underlined.
            if let Some(range) = self.alt_number.clone() {
                let (line, byte) = self.line_byte(range.start);
                let (_, end) = self.line_byte(range.end);
                if let (Some(a), Some(b)) = (
                    view.editor.position_rect(session, Position { line_index: line, byte_index: byte }),
                    view.editor.position_rect(session, Position { line_index: line, byte_index: end }),
                ) {
                    let r = Rect { pos: dvec2(origin.x + a.pos.x, origin.y + a.pos.y + a.size.y - 2.0), size: dvec2(b.pos.x - a.pos.x, 1.5) };
                    self.solid(cx, r, vec4(1.0, 0.48, 0.18, 0.9));
                }
            }
            cx.end_turtle();
        }
        self.draw_popup(cx, scope, clip);
    }

    /// A chip: its label (if any), then its value and control.
    fn draw_chip(&mut self, cx: &mut Cx2d, index: usize, rect: Rect, column: f64, label: &str) {
        let site = self.sites[index].clone();
        let written = self.source.get(site.range.clone()).unwrap_or_default().to_string();
        let hot = self.hover == Some(index) || self.popup_site() == Some(index) || self.focus == Some(index);
        let flash = self.focus == Some(index);
        let edge = if hot { vec4(1.0, 0.48, 0.18, 1.0) } else { vec4(1.0, 1.0, 1.0, 0.10) };
        self.boxed(cx, rect, vec4(1.0, 1.0, 1.0, if flash { 0.14 } else if hot { 0.09 } else { 0.045 }), edge, 4.0);
        let mut x = rect.pos.x + 6.0;
        let ty = rect.pos.y + (rect.size.y - 10.0) * 0.5;
        if !label.is_empty() {
            self.draw_glyph.color = vec4(0.55, 0.58, 0.63, 1.0);
            self.draw_glyph.draw_abs(cx, dvec2(x, ty + 1.0), label);
            x += label.chars().count() as f64 * column * 0.82 + 8.0;
        }
        let ink = if hot { vec4(1.0, 0.62, 0.32, 1.0) } else { vec4(0.88, 0.9, 0.93, 1.0) };
        let value_rect = Rect { pos: dvec2(x, rect.pos.y + 2.0), size: dvec2((rect.pos.x + rect.size.x - x - 4.0).max(8.0), rect.size.y - 4.0) };
        match &site.control {
            Control::Color(_) => {
                let swatch = Rect { pos: value_rect.pos + dvec2(0.0, 1.0), size: dvec2(value_rect.size.y - 2.0, value_rect.size.y - 2.0) };
                self.draw_control(cx, swatch, &site.control, hot);
                self.draw_label.color = ink;
                self.draw_label.draw_abs(cx, dvec2(swatch.pos.x + swatch.size.x + 5.0, ty), &written);
            }
            Control::Ease { curve, .. } => {
                let thumb = Rect { pos: value_rect.pos + dvec2(0.0, 2.0), size: dvec2(column * 2.4, value_rect.size.y - 4.0) };
                self.curve(cx, thumb, curve, ink);
                self.draw_label.color = ink;
                self.draw_label.draw_abs(cx, dvec2(thumb.pos.x + thumb.size.x + 5.0, ty), written.trim_start_matches('@'));
            }
            Control::Words { text } => {
                let max = ((value_rect.size.x / column) as usize).max(4);
                let mut shown: String = text.replace('\n', " ⏎ ").chars().take(max.saturating_sub(2)).collect();
                if text.chars().count() > max.saturating_sub(2) {
                    shown.push('…');
                }
                self.draw_label.color = ink;
                self.draw_label.draw_abs(cx, dvec2(value_rect.pos.x, ty), &format!("“{shown}”"));
            }
            Control::Choice { .. } => {
                self.draw_label.color = ink;
                self.draw_label.draw_abs(cx, dvec2(value_rect.pos.x, ty), written.trim_start_matches('@').trim_matches('"'));
                self.draw_ctl.shape = 3.0;
                self.draw_ctl.edge = ink;
                self.draw_ctl.draw_abs(cx, Rect { pos: dvec2(rect.pos.x + rect.size.x - 16.0, rect.pos.y), size: dvec2(12.0, rect.size.y) });
            }
            Control::Data { .. } | Control::Swatches { .. } | Control::Table { .. } | Control::Point(_) | Control::Point3(_) | Control::Direction(_) | Control::Path3 { .. } => {
                self.draw_value_chip(cx, index, value_rect, hot);
            }
            Control::Number => {
                let shown = match &self.gesture {
                    Some(g) if g.span.start == site.range.start => g.last.clone(),
                    _ => written.clone(),
                };
                self.draw_label.color = ink;
                self.draw_label.draw_abs(cx, dvec2(value_rect.pos.x, ty), &shown);
            }
        }
        self.hots.push((rect, index));
    }

    /// Focus the value at byte `at` (its chip or control lights up): what a
    /// click on the picture lands on.
    pub fn focus_at(&mut self, at: Option<usize>) {
        self.focus = at.and_then(|at| self.sites.iter().position(|s| s.range.start <= at && at <= s.range.end));
    }

    fn popup_site(&self) -> Option<usize> {
        self.popup.as_ref().map(Popup::site)
    }

    fn solid(&mut self, cx: &mut Cx2d, rect: Rect, color: Vec4f) {
        self.draw_ctl.shape = 1.0;
        self.draw_ctl.fill = color;
        self.draw_ctl.draw_abs(cx, rect);
    }

    fn boxed(&mut self, cx: &mut Cx2d, rect: Rect, fill: Vec4f, edge: Vec4f, radius: f32) {
        self.draw_ctl.shape = 0.0;
        self.draw_ctl.fill = fill;
        self.draw_ctl.edge = edge;
        self.draw_ctl.stroke = 1.0;
        self.draw_ctl.radius = radius;
        self.draw_ctl.draw_abs(cx, rect);
    }

    /// A curve over 0..1 drawn in `rect` as columns a point wide.
    fn curve(&mut self, cx: &mut Cx2d, rect: Rect, curve: &[f32], ink: Vec4f) {
        if curve.len() < 2 {
            return;
        }
        let lo = curve.iter().copied().fold(0.0f32, f32::min);
        let hi = curve.iter().copied().fold(1.0f32, f32::max);
        let span = (hi - lo).max(1e-3) as f64;
        let y_of = |v: f32| rect.pos.y + rect.size.y - ((v - lo) as f64 / span) * rect.size.y;
        let n = rect.size.x.max(2.0) as usize;
        let sample = |x: usize| {
            let u = x as f64 / (n - 1).max(1) as f64 * (curve.len() - 1) as f64;
            let i = (u.floor() as usize).min(curve.len() - 2);
            let f = (u - i as f64) as f32;
            curve[i] * (1.0 - f) + curve[i + 1] * f
        };
        let mut prev = y_of(sample(0));
        for x in 0..n {
            let y = y_of(sample(x));
            let (top, bottom) = (y.min(prev), y.max(prev));
            self.solid(cx, Rect { pos: dvec2(rect.pos.x + x as f64, top - 0.6), size: dvec2(1.0, (bottom - top).max(0.0) + 1.2) }, ink);
            prev = y;
        }
    }

    fn draw_control(&mut self, cx: &mut Cx2d, rect: Rect, control: &Control, hot: bool) {
        let edge = if hot { vec4(1.0, 0.48, 0.18, 1.0) } else { vec4(1.0, 1.0, 1.0, 0.22) };
        let ground = vec4(1.0, 1.0, 1.0, if hot { 0.12 } else { 0.06 });
        match control {
            Control::Color(rgba) => {
                let side = rect.size.y.min(rect.size.x);
                let r = Rect { pos: dvec2(rect.pos.x + (rect.size.x - side) * 0.5, rect.pos.y), size: dvec2(side, side) };
                self.draw_ctl.shape = 2.0;
                self.draw_ctl.fill = vec4(rgba[0], rgba[1], rgba[2], rgba[3]);
                self.draw_ctl.edge = edge;
                self.draw_ctl.stroke = 1.0;
                self.draw_ctl.radius = 2.5;
                self.draw_ctl.draw_abs(cx, r);
            }
            Control::Ease { curve, .. } => {
                self.boxed(cx, rect, ground, edge, 2.5);
                let inner = Rect { pos: rect.pos + dvec2(3.0, 3.0), size: rect.size - dvec2(6.0, 6.0) };
                self.curve(cx, inner, curve, if hot { vec4(1.0, 0.6, 0.3, 1.0) } else { vec4(0.86, 0.86, 0.67, 1.0) });
            }
            Control::Choice { .. } => {
                self.boxed(cx, rect, ground, edge, 2.5);
                self.draw_ctl.shape = 3.0;
                self.draw_ctl.edge = if hot { vec4(1.0, 0.6, 0.3, 1.0) } else { vec4(0.8, 0.82, 0.86, 1.0) };
                self.draw_ctl.draw_abs(cx, rect);
            }
            Control::Words { .. } => {
                self.boxed(cx, rect, ground, edge, 2.5);
                self.draw_glyph.color = if hot { vec4(1.0, 0.6, 0.3, 1.0) } else { vec4(0.8, 0.82, 0.86, 1.0) };
                self.draw_glyph.draw_abs(cx, dvec2(rect.pos.x + rect.size.x * 0.5 - 4.0, rect.pos.y + (rect.size.y - 10.0) * 0.5), "Aa");
            }
            // Drawn from their summaries (chips.rs).
            Control::Data { .. } | Control::Swatches { .. } | Control::Table { .. } | Control::Point(_) | Control::Point3(_) | Control::Direction(_) | Control::Path3 { .. } => {}
            Control::Number => {}
        }
    }

    fn menu_choices(&self, site: usize) -> Vec<Choice> {
        match self.sites.get(site).map(|s| &s.control) {
            Some(Control::Ease { choices, .. } | Control::Choice { choices }) => choices.clone(),
            _ => Vec::new(),
        }
    }

    fn draw_popup(&mut self, cx: &mut Cx2d, scope: &mut Scope, clip: Rect) {
        // No popup: the hovered chip's words, if it has some.
        let tip = match (&self.popup, self.hover, self.gesture.is_none()) {
            (None, Some(index), true) => self.hots.iter().find(|(_, i)| *i == index).map(|(r, _)| (index, *r)).filter(|(i, _)| self.chip_tip(*i).is_some()),
            _ => None,
        };
        let Some(popup) = self.popup.clone() else {
            if let Some((index, rect)) = tip {
                self.pop_list.begin_overlay_reuse(cx);
                let size = cx.current_pass_size();
                cx.begin_root_turtle(size, Layout::flow_down());
                self.draw_chip_tip(cx, index, rect);
                cx.end_pass_sized_turtle();
                self.pop_list.end(cx);
            }
            return;
        };
        self.pop_list.begin_overlay_reuse(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
        match popup {
            Popup::Wheel { rect, site, item, .. } => {
                self.boxed(cx, rect, vec4(0.08, 0.09, 0.11, 1.0), vec4(1.0, 1.0, 1.0, 0.16), 6.0);
                let walk = Walk { abs_pos: Some(rect.pos + dvec2(8.0, 8.0)), width: Size::Fixed(rect.size.x - 16.0), height: Size::Fixed(rect.size.x - 16.0), ..Walk::default() };
                self.wheel.draw_walk(cx, scope, walk).ok();
                if item.is_some() {
                    // The list's other colours: a click moves to one.
                    let start = self.sites.get(site).map_or(0, |s| s.range.start);
                    let colors: Vec<[f32; 4]> = data::color_items(&self.source, start).into_iter().map(|(_, c)| c).collect();
                    let strip = wheel_strip(rect);
                    self.strip_hits.retain(|(_, s, _)| *s != site);
                    let n = colors.len().max(1);
                    let w = (strip.size.x / n as f64).min(22.0);
                    for (k, c) in colors.iter().enumerate() {
                        let r = Rect { pos: dvec2(strip.pos.x + k as f64 * w, strip.pos.y), size: dvec2(w - 2.0, strip.size.y) };
                        let on = item == Some(k);
                        self.draw_ctl.shape = 2.0;
                        self.draw_ctl.fill = vec4(c[0], c[1], c[2], c[3]);
                        self.draw_ctl.edge = if on { vec4(1.0, 0.48, 0.18, 1.0) } else { vec4(1.0, 1.0, 1.0, 0.18) };
                        self.draw_ctl.stroke = if on { 1.5 } else { 1.0 };
                        self.draw_ctl.radius = 2.5;
                        self.draw_ctl.draw_abs(cx, r);
                    }
                }
            }
            Popup::Menu { site, rect, hover, top, .. } => {
                let choices = self.menu_choices(site);
                let current = self.sites.get(site).map(|s| self.source[s.range.clone()].to_string()).unwrap_or_default();
                self.boxed(cx, rect, vec4(0.08, 0.09, 0.11, 1.0), vec4(1.0, 1.0, 1.0, 0.16), 6.0);
                for (row, (i, choice)) in choices.iter().enumerate().skip(top).take(MENU_MAX_ROWS).enumerate() {
                    let r = Rect { pos: dvec2(rect.pos.x + 4.0, rect.pos.y + 4.0 + row as f64 * MENU_ROW), size: dvec2(rect.size.x - 8.0, MENU_ROW) };
                    if hover == Some(i) {
                        self.boxed(cx, r, vec4(1.0, 0.48, 0.18, 0.22), vec4(0.0, 0.0, 0.0, 0.0), 3.0);
                    }
                    let mut x = r.pos.x + 8.0;
                    if let Some(curve) = &choice.curve {
                        let c = Rect { pos: dvec2(x, r.pos.y + 4.0), size: dvec2(28.0, r.size.y - 8.0) };
                        self.curve(cx, c, curve, vec4(0.86, 0.86, 0.67, 1.0));
                        x += 36.0;
                    }
                    self.draw_label.color = if choice.text == current { vec4(1.0, 0.6, 0.3, 1.0) } else { vec4(0.86, 0.88, 0.9, 1.0) };
                    self.draw_label.draw_abs(cx, dvec2(x, r.pos.y + (r.size.y - 11.0) * 0.5), &choice.label);
                }
                if choices.len() > MENU_MAX_ROWS {
                    let shown = format!("{}–{} of {} · scroll", top + 1, (top + MENU_MAX_ROWS).min(choices.len()), choices.len());
                    self.draw_glyph.color = vec4(0.55, 0.58, 0.62, 1.0);
                    self.draw_glyph.draw_abs(cx, dvec2(rect.pos.x + 12.0, rect.pos.y + rect.size.y - 16.0), &shown);
                }
            }
            Popup::Plot { site, rect, hover, drag, pairs } => self.draw_plot_popup(cx, site, rect, hover, drag.as_ref(), pairs),
            Popup::Grid { site, rect, grid } => self.draw_grid_popup(cx, site, rect, &grid),
            Popup::Pad { site, rect, pad } => self.draw_pad_popup(cx, site, rect, &pad),
            Popup::Space { site, rect, space } => self.draw_space_popup(cx, site, rect, &space),
            Popup::Words { rect, focus, site } => {
                let walk = Walk { abs_pos: Some(rect.pos), width: Size::Fixed(rect.size.x), height: Size::Fixed(rect.size.y), ..Walk::default() };
                self.words.draw_walk(cx, scope, walk).ok();
                if focus {
                    self.words.take_key_focus(cx);
                    self.popup = Some(Popup::Words { rect, focus: false, site });
                }
            }
        }
        let _ = clip;
        cx.end_pass_sized_turtle();
        self.pop_list.end(cx);
    }

    fn next_gesture(&mut self) -> u64 {
        self.gestures += 1;
        self.gestures
    }

    /// A step of the gesture on the value at `span`: its new text.
    fn write(&mut self, out: &mut CodeValuesResponse, text: String, done: bool) {
        let Some(gesture) = self.gesture.as_mut() else { return };
        if text == gesture.last && !done {
            return;
        }
        let (start, end) = (gesture.span.start, gesture.span.end);
        out.edits.push(Edit { changes: vec![Change::Splice(Splice { file: 0, range: start..end, text: text.clone() })], gesture: gesture.id, done });
        // The model's copy follows at once, so the next step of a drag finds
        // the value where it now is.
        if end <= self.source.len() {
            self.source.replace_range(start..end, &text);
            self.line_starts = std::iter::once(0).chain(self.source.match_indices('\n').map(|(i, _)| i + 1)).collect();
            // What was read of long literals moves with it, and the values
            // after it (until the host gives the sites again).
            self.caches.spliced(&self.source, start, end, &text);
            let delta = text.len() as isize - (end - start) as isize;
            let moved = |at: usize| (at as isize + delta).max(0) as usize;
            for site in &mut self.sites {
                if site.range.start >= end {
                    site.range = moved(site.range.start)..moved(site.range.end);
                } else if site.range.end >= end {
                    site.range.end = moved(site.range.end);
                }
            }
        }
        gesture.span = start..start + text.len();
        gesture.last = text;
        if done {
            self.gesture = None;
        }
    }

    fn open(&mut self, cx: &mut Cx, index: usize, rect: Rect, press: DVec2, out: &mut CodeValuesResponse) {
        let Some(site) = self.sites.get(index).cloned() else { return };
        out.focus = Some(site.range.start);
        if site.control == Control::Number {
            // A number's chip drags: up or right raises it.
            let literal = self.source[site.range.clone()].to_string();
            if let Ok(base) = literal.trim().parse::<f64>() {
                let decimals = literal.split('.').nth(1).map_or(0, str::len);
                let id = self.next_gesture();
                self.gesture = Some(Gesture { id, span: site.range.clone(), last: literal.clone(), original: literal, scrub: Some((press, base, decimals)) });
            }
            return;
        }
        let id = self.next_gesture();
        let written = self.source[site.range.clone()].to_string();
        self.gesture = Some(Gesture { id, span: site.range.clone(), last: written.clone(), original: written, scrub: None });
        let clip = self.clip;
        let below = move |w: f64, h: f64| {
            let x = rect.pos.x.min(clip.pos.x + clip.size.x - w - 4.0).max(clip.pos.x);
            let y = if rect.pos.y + rect.size.y + 4.0 + h <= clip.pos.y + clip.size.y { rect.pos.y + rect.size.y + 4.0 } else { (rect.pos.y - 4.0 - h).max(clip.pos.y) };
            Rect { pos: dvec2(x, y), size: dvec2(w, h) }
        };
        self.popup = match &site.control {
            Control::Color(rgba) => {
                let [h, s, v] = rgb_to_hsv([rgba[0], rgba[1], rgba[2]]);
                self.wheel.set_hsv(cx, h, s, v);
                Some(Popup::Wheel { site: index, rect: below(184.0, 184.0), alpha: rgba[3], item: None })
            }
            Control::Swatches { .. } => {
                // The swatch pressed (the first when the press missed them).
                let items = data::color_items(&self.source, site.range.start);
                let k = self.strip_hits.iter().find(|(r, s, _)| *s == index && r.contains(press)).map_or(0, |(_, _, k)| *k);
                let Some((range, rgba)) = items.get(k).cloned() else {
                    self.gesture = None;
                    return;
                };
                let written = self.source[range.clone()].to_string();
                self.gesture = Some(Gesture { id, span: range, last: written.clone(), original: written, scrub: None });
                let [h, s, v] = rgb_to_hsv([rgba[0], rgba[1], rgba[2]]);
                self.wheel.set_hsv(cx, h, s, v);
                Some(Popup::Wheel { site: index, rect: below(184.0, 184.0 + WHEEL_STRIP), alpha: rgba[3], item: Some(k) })
            }
            Control::Table { .. } => {
                self.gesture = None;
                let mut grid = grid::GridState::new(None);
                let model = self.grid_model(index, None, 0);
                let Some(model) = model else { return };
                grid.col_w = self.grid_col_w(cx, &model);
                let size = grid::grid_size(&model, grid.col_w);
                Some(Popup::Grid { site: index, rect: below(size.x, size.y), grid })
            }
            Control::Point(v) => {
                self.gesture = None;
                let items = data::vector_items(&self.source, site.range.start);
                let v = if items.len() == 2 { [items[0].1, items[1].1] } else { [v[0] as f64, v[1] as f64] };
                Some(Popup::Pad { site: index, rect: below(pad::PAD_W, pad::PAD_H), pad: pad::PadState::around(v) })
            }
            Control::Point3(_) | Control::Direction(_) | Control::Path3 { .. } => {
                self.gesture = None;
                let kind = match &site.control {
                    Control::Point3(_) => gizmo::SpaceKind::Point,
                    Control::Direction(_) => gizmo::SpaceKind::Direction,
                    _ => gizmo::SpaceKind::Path,
                };
                if self.lines.is_none() {
                    self.lines = makepad_render_lines::LineRenderer::new(cx);
                }
                // A new view: drawn afresh.
                self.space_drawn = None;
                let space = gizmo::SpaceState::open(kind, &self.source, site.range.start);
                Some(Popup::Space { site: index, rect: below(gizmo::SPACE_W, gizmo::SPACE_H), space: Box::new(space) })
            }
            Control::Ease { choices, .. } | Control::Choice { choices } => {
                let current = &self.source[site.range.clone()];
                let at = choices.iter().position(|c| c.text == current).unwrap_or(0);
                let rows = choices.len().min(MENU_MAX_ROWS);
                let extra = if choices.len() > MENU_MAX_ROWS { 18.0 } else { 0.0 };
                Some(Popup::Menu { site: index, rect: below(MENU_W, rows as f64 * MENU_ROW + 8.0 + extra), hover: Some(at), top: at.saturating_sub(MENU_MAX_ROWS / 2).min(choices.len().saturating_sub(MENU_MAX_ROWS)), armed: false, from: press})
            }
            Control::Number => None,
            Control::Data { values, pairs, .. } => {
                // Items are dragged one at a time, each its own gesture.
                self.gesture = None;
                let pairs = *pairs && values.len() % 2 == 0;
                Some(Popup::Plot { site: index, rect: below(PLOT_W, if pairs { PLOT_H * 1.6 } else { PLOT_H }), hover: None, drag: None, pairs })
            }
            Control::Words { text } => {
                self.words.set_text(cx, text);
                let lines = text.split('\n').count().clamp(2, 8);
                self.words_settled = false;
                Some(Popup::Words { site: index, rect: below(360.0, lines as f64 * 17.0 + 16.0), focus: true })
            }
        };
    }

    fn close(&mut self, out: &mut CodeValuesResponse) {
        if let Some(gesture) = self.gesture.clone() {
            if gesture.scrub.is_none() {
                // The gesture's last step: what is written now, done.
                self.write(out, gesture.last.clone(), true);
            }
        }
        self.popup = None;
        self.gesture = None;
    }

    /// Every event goes here before the editor sees it.
    /// `view`: the code the controls sit in (`None` when the host places
    /// them itself, [`Self::draw_placed`]).
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope, view: Option<&CodeView>) -> CodeValuesResponse {
        // The release of a key a popup took is the popup's too.
        if let Event::KeyUp(k) = event {
            if self.swallow_up == Some(k.key_code) {
                self.swallow_up = None;
                return CodeValuesResponse { consumed: true, ..Default::default() };
            }
        }
        let had_popup = self.popup.is_some();
        let mut r = self.handle_event_inner(cx, event, scope, view);
        if had_popup {
            // While a popup is open it has the keyboard: Escape, Enter, Tab,
            // the arrows and typing never reach the host (nor their release,
            // after the popup closed on the press).
            match event {
                Event::KeyDown(k) => {
                    r.consumed = true;
                    self.swallow_up = Some(k.key_code);
                }
                Event::KeyUp(_) | Event::TextInput(_) | Event::TextCopy(_) | Event::TextCut(_) => r.consumed = true,
                _ => {}
            }
        }
        r
    }

    /// Whether the controls have the keyboard (a popup is open): a host
    /// with its own key shortcuts leaves keys to them while this is true.
    pub fn has_keyboard(&self) -> bool {
        self.popup.is_some()
    }

    fn handle_event_inner(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope, view: Option<&CodeView>) -> CodeValuesResponse {
        let mut r = CodeValuesResponse::default();
        // The open popup has the pointer and the keys.
        match self.popup.clone() {
            Some(Popup::Wheel { rect, alpha, site, item }) => {
                // A list's colour: another swatch under the wheel moves to it.
                if let (Some(current), Event::MouseDown(e)) = (item, event) {
                    let strip = wheel_strip(rect);
                    if strip.contains(e.abs) {
                        let start = self.sites.get(site).map_or(0, |s| s.range.start);
                        let items = data::color_items(&self.source, start);
                        let w = (strip.size.x / items.len().max(1) as f64).min(22.0);
                        let k = ((e.abs.x - strip.pos.x) / w).floor() as usize;
                        if k != current {
                            if let Some((range, rgba)) = items.get(k).cloned() {
                                if let Some(g) = self.gesture.clone() {
                                    if g.last != g.original {
                                        self.write(&mut r, g.last, true);
                                    }
                                }
                                let id = self.next_gesture();
                                let written = self.source[range.clone()].to_string();
                                r.focus = Some(range.start);
                                self.gesture = Some(Gesture { id, span: range, last: written.clone(), original: written, scrub: None });
                                let [h, s, v] = rgb_to_hsv([rgba[0], rgba[1], rgba[2]]);
                                self.wheel.set_hsv(cx, h, s, v);
                                self.popup = Some(Popup::Wheel { rect, alpha: rgba[3], site, item: Some(k) });
                            }
                        }
                        r.consumed = true;
                        r.redraw = true;
                        self.pop_list.redraw(cx);
                        return r;
                    }
                }
                let uid = self.wheel.widget_uid();
                self.wheel.handle_event(cx, event, scope);
                if let Event::Actions(actions) = event {
                    for action in actions.filter_widget_actions_cast::<ColorWheelAction>(uid) {
                        let (hsv, done) = match action {
                            ColorWheelAction::Changed(hsv) => (hsv, false),
                            ColorWheelAction::Ended(hsv) => (hsv, true),
                            ColorWheelAction::None => continue,
                        };
                        let [cr, cg, cb] = hsv_to_rgb(hsv[0], hsv[1], hsv[2]);
                        // Each release is a step of the same gesture; the
                        // popup closing ends it.
                        let _ = done;
                        self.write(&mut r, fmt_color([cr, cg, cb, alpha]), false);
                        r.redraw = true;
                    }
                }
                match event {
                    Event::MouseDown(e) if !rect.contains(e.abs) => {
                        self.close(&mut r);
                        r.consumed = true;
                        r.redraw = true;
                    }
                    Event::KeyDown(k) if k.key_code == KeyCode::Escape => {
                        // Back to the colour as it was, and closed.
                        self.revert(&mut r);
                        r.consumed = true;
                        r.redraw = true;
                    }
                    Event::KeyDown(k) if k.key_code == KeyCode::ReturnKey => {
                        self.close(&mut r);
                        r.consumed = true;
                        r.redraw = true;
                    }
                    Event::MouseDown(_) | Event::MouseMove(_) | Event::MouseUp(_) | Event::Scroll(_) | Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_) => r.consumed = true,
                    _ => {}
                }
                if r.redraw {
                    self.pop_list.redraw(cx);
                }
                return r;
            }
            Some(Popup::Menu { site, rect, hover, top, armed, from }) => {
                let choices = self.menu_choices(site);
                let row_at = |abs: DVec2| -> Option<usize> {
                    if !rect.contains(abs) {
                        return None;
                    }
                    let row = ((abs.y - rect.pos.y - 4.0) / MENU_ROW).floor();
                    (row >= 0.0).then(|| top + row as usize).filter(|i| *i < choices.len() && *i < top + MENU_MAX_ROWS)
                };
                let choose = |this: &mut Self, i: usize, out: &mut CodeValuesResponse| {
                    if let Some(choice) = choices.get(i) {
                        this.write(out, choice.text.clone(), true);
                    }
                    this.popup = None;
                    this.gesture = None;
                };
                match event {
                    Event::MouseMove(e) => {
                        let armed = armed || (e.abs - from).length() > 4.0;
                        let over = row_at(e.abs);
                        if armed != matches!(self.popup, Some(Popup::Menu { armed: true, .. })) {
                            self.popup = Some(Popup::Menu { site, rect, hover, top, armed, from });
                        }
                        if over != hover && over.is_some() {
                            self.popup = Some(Popup::Menu { site, rect, hover: over, top, armed, from });
                            // The value follows the pointer (a live preview).
                            if let Some(choice) = over.and_then(|i| choices.get(i)) {
                                self.write(&mut r, choice.text.clone(), false);
                            }
                            r.redraw = true;
                        }
                        r.consumed = true;
                    }
                    Event::MouseUp(e) => {
                        // The release of the press that opened it chooses
                        // nothing (the menu may lie under the pointer).
                        if armed || (e.abs - from).length() > 4.0 {
                            if let Some(i) = row_at(e.abs) {
                                choose(self, i, &mut r);
                                r.redraw = true;
                            }
                        }
                        if let Some(Popup::Menu { site, rect, hover, top, from, .. }) = self.popup.clone() {
                            self.popup = Some(Popup::Menu { site, rect, hover, top, armed: true, from });
                        }
                        r.consumed = true;
                    }
                    Event::MouseDown(e) => {
                        if !rect.contains(e.abs) {
                            // Away: back to what was written, and closed.
                            self.revert(&mut r);
                        }
                        r.consumed = true;
                        r.redraw = true;
                    }
                    Event::Scroll(e) if rect.contains(e.abs) => {
                        let max_top = choices.len().saturating_sub(MENU_MAX_ROWS);
                        let next = if e.scroll.y > 0.0 { (top + 1).min(max_top) } else { top.saturating_sub(1) };
                        self.popup = Some(Popup::Menu { site, rect, hover, top: next, armed, from });
                        r.consumed = true;
                        r.redraw = true;
                    }
                    Event::KeyDown(k) => {
                        let n = choices.len();
                        let cur = hover.unwrap_or(0);
                        let next = match k.key_code {
                            KeyCode::ArrowDown => Some((cur + 1).min(n.saturating_sub(1))),
                            KeyCode::ArrowUp => Some(cur.saturating_sub(1)),
                            KeyCode::ReturnKey => {
                                choose(self, cur, &mut r);
                                None
                            }
                            KeyCode::Escape => {
                                self.revert(&mut r);
                                None
                            }
                            _ => None,
                        };
                        if let Some(next) = next {
                            let top = if next < top { next } else if next >= top + MENU_MAX_ROWS { next + 1 - MENU_MAX_ROWS } else { top };
                            self.popup = Some(Popup::Menu { site, rect, hover: Some(next), top, armed, from });
                            if let Some(choice) = choices.get(next) {
                                self.write(&mut r, choice.text.clone(), false);
                            }
                        }
                        r.consumed = true;
                        r.redraw = true;
                    }
                    Event::TextInput(_) => r.consumed = true,
                    _ => {}
                }
                if r.redraw {
                    self.pop_list.redraw(cx);
                }
                return r;
            }
            Some(Popup::Plot { site, rect, hover, drag, pairs }) => return self.plot_event(cx, event, site, rect, hover, drag, pairs),
            Some(Popup::Grid { site, rect, grid }) => return self.grid_event(cx, event, site, rect, grid),
            Some(Popup::Pad { site, rect, pad }) => return self.pad_event(cx, event, site, rect, pad),
            Some(Popup::Space { site, rect, space }) => return self.space_event(cx, event, site, rect, *space),
            Some(Popup::Words { rect, .. }) => {
                let uid = self.words.widget_uid();
                self.words.handle_event(cx, event, scope);
                if let Event::Actions(actions) = event {
                    for action in actions.filter_widget_actions(uid) {
                        match action.cast::<TextInputAction>() {
                            TextInputAction::Changed(text) => {
                                self.write(&mut r, quote(&text), false);
                                r.redraw = true;
                            }
                            TextInputAction::Escaped => {
                                self.revert(&mut r);
                                r.redraw = true;
                            }
                            // The release of the press that opened it may
                            // take the focus: it is taken back.
                            TextInputAction::KeyFocusLost if !self.words_settled => {
                                self.words.take_key_focus(cx);
                            }
                            TextInputAction::KeyFocusLost => {
                                self.close(&mut r);
                                r.redraw = true;
                            }
                            TextInputAction::Returned(_, modifiers) if modifiers.logo || modifiers.control => {
                                self.close(&mut r);
                                r.redraw = true;
                            }
                            _ => {}
                        }
                    }
                }
                if let Event::MouseUp(_) = event {
                    if !self.words_settled {
                        self.words_settled = true;
                        self.words.take_key_focus(cx);
                    }
                }
                match event {
                    Event::MouseDown(e) if !rect.contains(e.abs) => {
                        self.close(&mut r);
                        r.consumed = true;
                        r.redraw = true;
                    }
                    Event::MouseDown(_) | Event::MouseMove(_) | Event::MouseUp(_) | Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_) | Event::TextCopy(_) | Event::TextCut(_) => r.consumed = true,
                    _ => {}
                }
                if r.redraw {
                    self.pop_list.redraw(cx);
                }
                return r;
            }
            None => {}
        }
        // A number scrubbed in the code.
        if let Some(gesture) = self.gesture.clone() {
            if let Some((start, base, decimals)) = gesture.scrub {
                let (abs, fine, done) = match event {
                    Event::MouseMove(e) => (e.abs, e.modifiers.shift, false),
                    Event::MouseUp(e) => (e.abs, e.modifiers.shift, true),
                    _ => return r,
                };
                // Up or right raises it (across counts half).
                let travel = (start.y - abs.y) + (abs.x - start.x) * 0.5;
                let steps = travel / SCRUB_PX * if fine { 0.1 } else { 1.0 };
                let text = within(&self.sites, gesture.span.start, scrubbed(base, decimals, steps), decimals);
                self.write(&mut r, text, done);
                if done {
                    self.gesture = None;
                }
                r.consumed = true;
                r.redraw = true;
                return r;
            }
        }
        match event {
            Event::MouseMove(e) => {
                let over = self.hots.iter().find(|(rect, _)| rect.contains(e.abs) && self.clip.contains(e.abs)).map(|(_, i)| *i);
                if over != self.hover {
                    self.hover = over;
                    r.redraw = true;
                }
                if over.is_some() {
                    cx.set_cursor(MouseCursor::Hand);
                }
                let number = (e.modifiers.alt && self.fresh && self.clip.contains(e.abs)).then(|| self.offset_at(cx, view, e.abs)).flatten().and_then(|at| number_at(&self.source, at));
                if number != self.alt_number {
                    self.alt_number = number;
                    r.redraw = true;
                }
                if self.alt_number.is_some() {
                    cx.set_cursor(MouseCursor::EwResize);
                }
            }
            Event::MouseDown(e) if self.fresh => {
                if let Some((rect, index)) = self.hots.iter().find(|(rect, _)| rect.contains(e.abs) && self.clip.contains(e.abs)).copied() {
                    self.open(cx, index, rect, e.abs, &mut r);
                    r.consumed = true;
                    r.redraw = true;
                } else if e.modifiers.alt && self.clip.contains(e.abs) {
                    if let Some(range) = self.offset_at(cx, view, e.abs).and_then(|at| number_at(&self.source, at)) {
                        let literal = self.source[range.clone()].to_string();
                        if let Ok(base) = literal.parse::<f64>() {
                            let decimals = literal.split('.').nth(1).map_or(0, str::len);
                            let id = self.next_gesture();
                            r.focus = Some(range.start);
                            self.gesture = Some(Gesture { id, span: range, last: literal.clone(), original: literal, scrub: Some((e.abs, base, decimals)) });
                            r.consumed = true;
                        }
                    }
                }
            }
            _ => {}
        }
        r
    }

    /// Back to what was written when the gesture began, and closed.
    fn revert(&mut self, out: &mut CodeValuesResponse) {
        self.popup = None;
        let Some(gesture) = self.gesture.clone() else { return };
        self.write(out, gesture.original.clone(), true);
        self.gesture = None;
    }

    /// The source byte under `abs` in `view`.
    fn offset_at(&self, cx: &Cx, view: Option<&CodeView>, abs: DVec2) -> Option<usize> {
        let view = view?;
        let session = view.session.as_ref()?;
        let origin = view.editor.area().rect(cx).pos;
        let hit = view.editor.hit_test(session, abs - origin)?;
        let at = self.source.split('\n').take(hit.position.line_index).map(|l| l.len() + 1).sum::<usize>() + hit.position.byte_index;
        Some(at.min(self.source.len()))
    }

    /// The site whose value starts at byte `start` (what the host placed).
    /// Open the editor of the value starting at byte `start`, as a press on
    /// its control would (when its control was drawn this frame): a click
    /// on the picture edits the words it shows at once. Whether it opened.
    pub fn open_editor(&mut self, cx: &mut Cx, start: usize) -> bool {
        let Some(index) = self.site_at(start) else { return false };
        let Some((rect, _)) = self.hots.iter().find(|(_, i)| *i == index).copied() else { return false };
        let mut out = CodeValuesResponse::default();
        self.open(cx, index, rect, rect.pos + rect.size * 0.5, &mut out);
        true
    }

    pub fn site_at(&self, start: usize) -> Option<usize> {
        let i = self.sites.partition_point(|s| s.range.start < start);
        (self.sites.get(i)?.range.start == start).then_some(i)
    }

    /// Draw the controls where the host puts them, `placed` (site, rect),
    /// each as a chip without its label (the host writes the name), then
    /// any popup above everything. `clip`: the area they live in.
    pub fn draw_placed(&mut self, cx: &mut Cx2d, scope: &mut Scope, placed: &[(usize, Rect)], column: f64, clip: Rect) {
        self.hots.clear();
        self.strip_hits.clear();
        self.clip = clip;
        self.fresh = true;
        for &(index, rect) in placed {
            if index < self.sites.len() {
                self.draw_chip(cx, index, rect, column, "");
            }
        }
        self.draw_popup(cx, scope, clip);
    }

    /// How many times a 3D view's picture has been drawn: only while its
    /// popup is open and what it shows changed.
    pub fn view_renders(&self) -> u64 {
        self.space_renders
    }

    /// Whether a popup or a gesture is open (the host keeps its sites).
    pub fn busy(&self) -> bool {
        self.popup.is_some() || self.gesture.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_found_outside_strings_names_and_colours() {
        let src = "Text{text: \"12 apples\" size: 48 at: vec2(-10, 2.5) fill: #ff5a36 // 99\n y: 3 - 1}";
        let at = |needle: &str| src.find(needle).unwrap();
        assert_eq!(number_at(src, at("12 apples")), None);
        assert_eq!(&src[number_at(src, at("48")).unwrap()], "48");
        assert_eq!(&src[number_at(src, at("10,")).unwrap()], "-10");
        assert_eq!(&src[number_at(src, at("2.5")).unwrap()], "2.5");
        assert_eq!(number_at(src, at("5a36")), None);
        assert_eq!(number_at(src, at("99")), None);
        assert_eq!(&src[number_at(src, at("1}")).unwrap()], "1");
        assert_eq!(number_at(src, at("vec2") + 3), None);
    }

    #[test]
    fn a_list_reads_as_items_and_writes_back_in_place() {
        let src = "let O_B = [0.238, -0.692,\n  1.147, 3, .5] // 9";
        let items = data_items(src, src.find('[').unwrap());
        assert_eq!(items.iter().map(|(r, _)| &src[r.clone()]).collect::<Vec<_>>(), vec!["0.238", "-0.692", "1.147", "3", ".5"]);
        assert_eq!(items[1].1, -0.692);
        assert_eq!(fmt_item(0.31234, "0.238", 2.0), "0.312");
        assert_eq!(fmt_item(4.4, "3", 10.0), "4.4");
    }

    #[test]
    fn a_scrub_keeps_the_precision_written() {
        assert_eq!(scrubbed(48.0, 0, 3.0), "51");
        assert_eq!(scrubbed(0.25, 2, -30.0), "-0.05");
        assert_eq!(scrubbed(0.05, 2, -5.0), "0.00");
        assert_eq!(scrubbed(1.5, 1, 0.4), "1.5");
        assert_eq!(scrubbed(120.5, 1, 2.0), "122.5");
    }

    #[test]
    fn typed_sites_pick_their_controls() {
        let src = "Synth{osc: @saw cutoff: 800 fill: #ff0000 words: \"hi\" ease: @ease_out}";
        let site = |kind: ValueKind, text: &str, choices: &[&str]| {
            let at = src.find(text).unwrap();
            ValueSite { kind, choices: choices.iter().map(|c| c.to_string()).collect(), span: Some((0, at..at + text.len())), range: makepad_edit_values::Range::hard(20.0, 2000.0), ..Default::default() }
        };
        let osc = place(src, &site(ValueKind::Choice, "@saw", &["saw", "square"])).unwrap();
        assert!(matches!(&osc.control, Control::Choice { choices } if choices[1].text == "@square" && choices[1].label == "square"));
        let cutoff = place(src, &site(ValueKind::Frequency, "800", &[])).unwrap();
        assert_eq!(cutoff.control, Control::Number);
        let at = cutoff.range.start;
        assert_eq!(within(&[cutoff], at, "5000".into(), 0), "2000");
        assert!(matches!(place(src, &site(ValueKind::Colour, "#ff0000", &[])).unwrap().control, Control::Color(c) if c[0] == 1.0));
        assert_eq!(place(src, &site(ValueKind::Text, "\"hi\"", &[])).unwrap().control, Control::Words { text: "hi".into() });
        let samples = "Env{gain: [0, 0.5, 1]}";
        let at = samples.find('[').unwrap();
        let gain = ValueSite { kind: ValueKind::Time, step: 0.25, span: Some((0, at..samples.len() - 1)), ..Default::default() };
        assert!(matches!(place(samples, &gain).unwrap().control, Control::Data { axis: DataAxis::Time { per_second, .. }, .. } if per_second == 4.0));
        let ease = place(src, &site(ValueKind::Ease, "@ease_out", &[])).unwrap();
        assert!(matches!(&ease.control, Control::Ease { curve, choices } if curve.len() == 25 && choices.len() > 10));
        // Not written in this file: no control.
        assert!(place(src, &ValueSite { kind: ValueKind::Number, ..Default::default() }).is_none());
    }

    #[test]
    fn words_quote_and_colours_format() {
        assert_eq!(quote("Two\nlines \"q\""), "\"Two\\nlines \\\"q\\\"\"");
        assert_eq!(fmt_color([1.0, 0.0, 0.5, 1.0]), "#ff0080");
        assert_eq!(fmt_color([0.0, 0.0, 0.0, 0.5]), "#00000080");
        let rgb = hsv_to_rgb(0.0, 1.0, 1.0);
        assert_eq!(rgb, [1.0, 0.0, 0.0]);
        let hsv = rgb_to_hsv([0.0, 0.5, 1.0]);
        let back = hsv_to_rgb(hsv[0], hsv[1], hsv[2]);
        assert!((back[1] - 0.5).abs() < 1e-5);
    }
}
