//! A document's editor panel: a small Splash program written into the
//! document itself, `let editor_panel = fn() { View{ … } }`, which the
//! document never calls. A host (the graphic desk, the SCORE page) runs it
//! in a `Splash` isolate (its own VM, sandboxed: no files, no processes, no
//! network) in an [`EditorPanel`] widget, so it shows as soon as the
//! document opens.
//!
//! The program reaches the document's code only through `mod.scene`:
//! - `scene.read(path)`: the value written at a path: a number, a bool,
//!   the words of a string, else the text as written (`#rrggbb`, `@name`);
//! - `scene.set(path, value)`: a new value there. While a finger is down in
//!   the panel the host holds it live (the running document takes it where
//!   it can), and the release writes it into the code: one undo step per
//!   gesture, like typing;
//! - `scene.swap(path, name)`: a function slot (or an instrument reference)
//!   names another of the same kind;
//! - `scene.replace(path, code)`: new code for the expression at a path,
//!   refused when it does not parse;
//! - `scene.ask(prompt, {target: path or "selection", mode: "replace",
//!   "variants" or "extend"})`: the document's agent asked to make or change
//!   something; returns an id, and `scene.ask_status(id)` says "working",
//!   "done: …" or "failed: …" (the panel widgets' host's `ask` and
//!   `ask_status`, the same path a widget's AI button takes).
//!
//! Paths are the code's own names, resolved each time
//! ([`ValueSource::resolve`]), so a panel survives edits of the code
//! around what it binds.
//!
//! A panel over several documents (an edit's, `let cut_panel`: the edit,
//! the graphics it shows and the modules they share) is given them with
//! [`set_docs`]; its paths name the document first
//! ([`PanelDoc::resolve_in`]: `edit.p_open.span`, `p_open:BONE`,
//! `look.POST.bloom`), and a set (`*:LYRIC_LOOK.font`, `edit.*.cut`)
//! reaches every member: a number moves each by as much as the control
//! moved from where the gesture began, anything else is written to each.
//! `scene.open(node)` asks the host to open a graphic's own panel.
//!
//! One host at a time: the host whose document is open calls
//! [`set_files`] when its text changes and [`Runtime::pump`] every turn;
//! the edits the program asked for come back as [`Splice`]s on its files.

use makepad_strict_json::{self as json, Value as Json};
use makepad_widgets::makepad_script::script_err_invalid_args;
use makepad_widgets::*;
use std::cell::{Cell, RefCell};
use std::ops::Range;
use makepad_value_pickers::panel::host::{PanelHost, PanelValue};
use std::any::Any;
use std::rc::Rc;

mod auto;
mod doc;
pub mod make;
pub use auto::{auto_modules, auto_panel, control as auto_control};
pub use doc::{fmt_num, fmt_text, Bound, File, PanelDoc, Swatch, Value, ValueKind, ValueSite, ValueSource};
use makepad_edit_values::Splice;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    let stage = mod.stage

    mod.widgets.PanelHandBase = #(PanelHand::register_widget(vm))
    /** A View whose panel widgets (outside an EditorPanel: a host's own
     * controls bound to paths) hold a gesture's values while a finger is
     * down in it and write them at the release, one undo step. */
    mod.widgets.PanelHand = set_type_default() do mod.widgets.PanelHandBase{}

    mod.widgets.EditorPanelBase = #(EditorPanel::register_widget(vm))
    /** A document's editor panel: its program, run in an isolate, under a
     * header with its state; with no program, an offer to make one. */
    mod.widgets.EditorPanel = set_type_default() do mod.widgets.EditorPanelBase{
        width: Fill height: Fill flow: Down
        // No header of its own: the panel's search box is its top (a remake
        // is asked in the scene's chat). Only a state worth saying shows here.
        View{width: Fill height: Fit flow: Right align: Align{y: 0.5}
            p_status := Label{width: Fill max_lines: 1 visible: false padding: Inset{left: 12 right: 8 top: 6 bottom: 2} text: "" draw_text +: {color: stage.muted text_style: theme.font_regular{font_size: 8}}}
            p_stop := StageButton{visible: false width: Fit height: 22 margin: Inset{right: 8} padding: Inset{left: 8 right: 8} text: "Stop"
                draw_text +: {text_style: theme.font_bold{font_size: 8}}}
        }
        p_empty := View{width: Fill height: Fill flow: Down spacing: 10 padding: Inset{left: 24 right: 24 top: 40} align: Align{x: 0.5}
            p_empty_text := Label{width: Fill text: "This document has no editor yet. Its agent can make one: controls for what this document is, written into it, editing its code."
                draw_text +: {color: stage.muted text_style: theme.font_regular{font_size: 9}}}
            p_make := StageButton{width: Fit height: 26 padding: Inset{left: 14 right: 14} text: "Make me an editor"
                draw_text +: {text_style: theme.font_bold{font_size: 9}}}
        }
        // A fold keeps the view where it is (empty space at the end until the
        // reader scrolls back); what opens is brought into view smoothly.
        p_scroll := ScrollYView{width: Fill height: Fill visible: false
            scroll_bars +: {scroll_bar_y +: {keep_on_shrink: true smoothing: 0.2}}
            p_splash := Splash{width: Fill height: Fit}
        }
    }
}

/// An edit the panel's program asked for.
#[derive(Clone, Debug)]
pub enum Edit {
    /// A value, written as the site's kind writes it (a number keeps its
    /// unit; a string is quoted).
    Set { path: String, value: Json },
    /// Code as written (`#ff8a3d`, `@saw`, `"words"`): a widget's value.
    SetCode { path: String, code: String },
    /// Colours moved at every place each is written (old as written, new).
    Recolour { map: Vec<([f64; 4], [f64; 4])> },
    Swap { path: String, name: String },
    Replace { path: String, code: String },
}

/// A text edit for the host: the splice (its file an index into the
/// files it gave). `gesture` groups a gesture's edits into one undo step;
/// `done` is its last (the release, or an edit outside a gesture).
#[derive(Clone, Debug, PartialEq)]
pub struct PanelEdit {
    pub splice: Splice,
    pub gesture: u64,
    pub done: bool,
    /// The path the panel named and the site's kind (a host that patches
    /// the running document live maps these to its sinks).
    pub path: String,
    pub kind: ValueKind,
}

/// A path resolved in one document, or why not (with the nearest paths
/// that exist).
fn resolve_one(doc: &dyn PanelDoc, files: &[File], path: &str) -> Result<ValueSite, String> {
    let values = doc.values(files);
    values.resolve(path).map_err(|why| {
        let near = values.nearest(path);
        if near.is_empty() { why } else { format!("{why} (paths near it: {})", near.join(", ")) }
    })
}

/// The files `read` resolves against: (name as imported, `""` for the
/// document; text; outline), and which document each is when they are
/// several ([`set_docs`]).
#[derive(Default)]
struct Scene {
    files: Vec<(String, String, Box<dyn Any>)>,
    table: Option<Box<dyn Any>>,
}

impl Scene {
    fn files(&self) -> Vec<File<'_>> {
        self.files.iter().map(|(name, text, code)| File { name, text, code: &**code }).collect()
    }
}

thread_local! {
    /// The document kind the panels edit ([`register`]).
    static DOC: RefCell<Option<Rc<dyn PanelDoc>>> = const { RefCell::new(None) };
    static SCENE: RefCell<Scene> = RefCell::new(Scene::default());
    static EDITS: RefCell<Vec<Edit>> = const { RefCell::new(Vec::new()) };
    /// A finger is down in a panel (a gesture: its edits are held live).
    static PRESSED: Cell<bool> = const { Cell::new(false) };
    /// The gesture's values so far, by path, as code: what `read` answers
    /// while the code has not been written yet.
    static HELD: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    /// The libraries every panel isolate gets ([`register`]).
    static ISOLATE_MODS: RefCell<Vec<fn(&mut ScriptVm)>> = const { RefCell::new(Vec::new()) };
    /// The desk whose document the panels read and write now.
    static OWNER: Cell<Option<&'static str>> = const { Cell::new(None) };
    static RELEASED: Cell<bool> = const { Cell::new(false) };
    /// Graphics the program asked to open (`scene.open`).
    static OPENS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The document kind the panels edit ([`register`] installs it).
pub fn install_doc(doc: Rc<dyn PanelDoc>) {
    DOC.with(|d| *d.borrow_mut() = Some(doc));
}

/// The installed document kind.
fn doc() -> Rc<dyn PanelDoc> {
    DOC.with(|d| d.borrow().clone()).expect("editor_panel::register installs the document kind first")
}

/// Take the panel for `owner` (a desk: its document is the one panels read
/// and write now). True when it changed hands: the new owner installs its
/// hosts ([`install_host`]) and gives its files ([`set_files`]).
pub fn claim(owner: &'static str) -> bool {
    OWNER.with(|o| o.replace(Some(owner)) != Some(owner))
}

/// The texts the program's `read` resolves against, when they change.
/// (name as imported, `""` for the document; text; the outline the document
/// kind made of it.)
pub fn set_files<O: Any>(files: Vec<(String, String, O)>) {
    let files = files.into_iter().map(|(name, text, code)| (name, text, Box::new(code) as Box<dyn Any>)).collect();
    SCENE.with(|s| *s.borrow_mut() = Scene { files, table: None });
}

/// Several documents' texts, and which is which: paths name their document
/// first ([`PanelDoc::resolve_in`]); `table` is the document kind's own.
pub fn set_docs<O: Any, T: Any>(files: Vec<(String, String, O)>, table: T) {
    let files = files.into_iter().map(|(name, text, code)| (name, text, Box::new(code) as Box<dyn Any>)).collect();
    SCENE.with(|s| *s.borrow_mut() = Scene { files, table: Some(Box::new(table)) });
}

/// Every place `path` reaches in `files` (one document: one place; several,
/// as [`set_docs`] said which is which: a set's members).
pub fn bounds(files: &[File], path: &str) -> Result<Vec<Bound>, String> {
    let doc = doc();
    SCENE.with(|s| match &s.borrow().table {
        Some(table) => doc.resolve_in(files, &**table, path),
        None => resolve_one(&*doc, files, path).map(|site| vec![Bound { site, scale: 1.0, member: String::new() }]),
    })
}

/// Every place `path` reaches in the texts the program reads.
pub fn scene_bounds(path: &str) -> Result<Vec<Bound>, String> {
    SCENE.with(|s| {
        let s = s.borrow();
        let files = s.files();
        let doc = doc();
        match &s.table {
            Some(table) => doc.resolve_in(&files, &**table, path),
            None => resolve_one(&*doc, &files, path).map(|site| vec![Bound { site, scale: 1.0, member: String::new() }]),
        }
    })
}

/// A graphic's own panel asked for (a scene card's click, `scene.open`).
pub fn open(node: &str) {
    OPENS.with(|o| o.borrow_mut().push(node.to_string()));
}

/// The graphics the program asked to open since the last call.
pub fn take_opens() -> Vec<String> {
    OPENS.with(|o| std::mem::take(&mut *o.borrow_mut()))
}

/// Queue an edit as the program's `scene` would (a host's own controls
/// bound to the panel's paths go through the same gesture rules).
pub fn queue(edit: Edit) {
    EDITS.with(|e| e.borrow_mut().push(edit));
}

/// A finger is down in the panel now.
pub fn pressed() -> bool {
    PRESSED.with(|p| p.get())
}

/// A gesture a host plays as a hand would (the control API's driven
/// gesture): its steps are held from here …
pub fn press() {
    PRESSED.with(|p| p.set(true));
}

/// … and written at this release, one undo step.
pub fn release() {
    if PRESSED.with(|p| p.replace(false)) {
        RELEASED.with(|r| r.set(true));
    }
}

/// The value at `path` as the program reads it: the gesture's value while
/// one is held there, else what the files given write.
pub fn read(path: &str) -> Result<Read, String> {
    if let Some(text) = HELD.with(|h| h.borrow().iter().find(|(p, _)| p == path).map(|(_, text)| text.clone())) {
        let digits = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
        return Ok(match (text.as_str(), digits.parse::<f64>()) {
            ("true", _) => Read::Bool(true),
            ("false", _) => Read::Bool(false),
            (_, Ok(n)) => Read::Number(n),
            _ => Read::Text(text),
        });
    }
    scene_bounds(path).map(|b| read_site(&b[0].site))
}

/// What a bound widget shows at `path` (it reads at every change of the
/// scene, a whole panel of them): the gesture's held value there, else the
/// one place the path resolves to, found once (no nearest paths looked for
/// when it does not: the widget only needs to know it is not there).
fn widget_read(path: &str) -> Option<PanelValue> {
    let code = match HELD.with(|h| h.borrow().iter().find(|(p, _)| p == path).map(|(_, text)| text.clone())) {
        Some(text) => text,
        None => SCENE.with(|s| {
            let s = s.borrow();
            let files = s.files();
            let doc = doc();
            match &s.table {
                Some(table) => doc.resolve_in(&files, &**table, path).ok().and_then(|mut b| (!b.is_empty()).then(|| b.swap_remove(0).site.text)),
                None => doc.values(&files).resolve(path).ok().map(|site| site.text),
            }
        })?,
    };
    let t = code.trim();
    // A keyed value shows its value at the playhead.
    if t.starts_with("keys(") {
        let at = makepad_value_pickers::panel::host::playhead().map_or(0.0, |p| p.0);
        if let Some(v) = doc().keys_sample(t, 0..t.len(), at) {
            return Some(PanelValue::Number(v));
        }
    }
    Some(match t {
        "true" => PanelValue::Bool(true),
        "false" => PanelValue::Bool(false),
        _ => match t.parse::<f64>() {
            Ok(n) if t.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') => PanelValue::Number(n),
            _ => PanelValue::Code(code),
        },
    })
}

/// A value as the program reads it.
#[derive(Clone, Debug, PartialEq)]
pub enum Read {
    Number(f64),
    Bool(bool),
    Text(String),
}

/// What a site holds, as `read` gives it.
pub fn read_site(site: &ValueSite) -> Read {
    match site.kind {
        kind if kind.is_numeric() => {
            let digits: String = site.text.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
            Read::Number(digits.parse::<f64>().unwrap_or(0.0))
        }
        ValueKind::Bool => Read::Bool(site.text == "true"),
        ValueKind::Text => Read::Text(
            makepad_widgets::makepad_script::syntax::parse_value(&site.text)
                .ok()
                .and_then(|n| match n.kind {
                    makepad_widgets::makepad_script::syntax::Kind::Str(t) => Some(t),
                    _ => None,
                })
                .unwrap_or_else(|| site.text.clone()),
        ),
        _ => Read::Text(site.text.clone()),
    }
}

fn string_arg(vm: &mut ScriptVm, value: ScriptValue) -> Option<String> {
    let mut out = None;
    vm.string_with(value, |_vm, s| out = Some(s.to_string()));
    out
}

/// The `scene` module of a panel's isolate.
fn scene_module(vm: &mut ScriptVm) {
    let scene = vm.new_module(id!(scene));
    // In the prelude too: a panel says `scene.set(…)`.
    makepad_widgets::script_eval!(vm, {
        mod.prelude.widgets = {..mod.prelude.widgets, scene: mod.scene}
    });
    vm.add_method(scene, id_lut!(read), script_args_def!(path = NIL), |vm, args| {
        let path = script_value!(vm, args.path);
        let Some(path) = string_arg(vm, path) else { return script_err_invalid_args!(vm.trap(), "a path is a string") };
        match read(&path) {
            Ok(Read::Number(n)) => ScriptValue::from_f64(n),
            Ok(Read::Bool(b)) => ScriptValue::from_bool(b),
            Ok(Read::Text(t)) => vm.new_string_with(|_vm, s| s.push_str(&t)).into(),
            Err(why) => script_err_invalid_args!(vm.trap(), "{}", why),
        }
    });
    vm.add_method(scene, id_lut!(set), script_args_def!(path = NIL, value = NIL), |vm, args| {
        let (path, value) = (script_value!(vm, args.path), script_value!(vm, args.value));
        let Some(path) = string_arg(vm, path) else { return script_err_invalid_args!(vm.trap(), "a path is a string") };
        let mut text = String::new();
        vm.bx.heap_ref().to_json_inner(value, &mut text);
        let value = json::parse_depth(text.as_bytes(), 8).unwrap_or(Json::Null);
        queue(Edit::Set { path, value });
        NIL
    });
    vm.add_method(scene, id_lut!(swap), script_args_def!(path = NIL, name = NIL), |vm, args| {
        let (path, name) = (script_value!(vm, args.path), script_value!(vm, args.name));
        let (Some(path), Some(name)) = (string_arg(vm, path), string_arg(vm, name)) else { return script_err_invalid_args!(vm.trap(), "swap(path, name): strings") };
        queue(Edit::Swap { path, name });
        NIL
    });
    vm.add_method(scene, id_lut!(ask), script_args_def!(prompt = NIL, opts = NIL), |vm, args| {
        let (prompt, opts) = (script_value!(vm, args.prompt), script_value!(vm, args.opts));
        let Some(prompt) = string_arg(vm, prompt) else { return script_err_invalid_args!(vm.trap(), "ask(prompt, {{target, mode}}): a prompt string") };
        let mut text = String::new();
        vm.bx.heap_ref().to_json_inner(opts, &mut text);
        let opts = json::parse_depth(text.as_bytes(), 4).unwrap_or(Json::Null);
        let field = |key: &str, default: &str| opts.get(key).and_then(Json::as_str).unwrap_or(default).to_string();
        match makepad_value_pickers::panel::host::ask(&prompt, &[field("target", "selection")], &field("mode", "replace")) {
            Some(id) => ScriptValue::from_f64(id as f64),
            None => script_err_invalid_args!(vm.trap(), "this document's agent cannot be asked here"),
        }
    });
    vm.add_method(scene, id_lut!(ask_status), script_args_def!(id = NIL), |vm, args| {
        let id = script_value!(vm, args.id).as_f64().unwrap_or(0.0) as u64;
        let text = makepad_value_pickers::panel::host::ask_status(id);
        vm.new_string_with(|_vm, s| s.push_str(&text)).into()
    });
    vm.add_method(scene, id_lut!(replace), script_args_def!(path = NIL, code = NIL), |vm, args| {
        let (path, code) = (script_value!(vm, args.path), script_value!(vm, args.code));
        let (Some(path), Some(code)) = (string_arg(vm, path), string_arg(vm, code)) else { return script_err_invalid_args!(vm.trap(), "replace(path, code): strings") };
        if let Err(error) = makepad_widgets::makepad_script::syntax::parse_value(&code) {
            return script_err_invalid_args!(vm.trap(), "the code does not parse: {}", error.message);
        }
        queue(Edit::Replace { path, code });
        NIL
    });
    vm.add_method(scene, id_lut!(open), script_args_def!(node = NIL), |vm, args| {
        let node = script_value!(vm, args.node);
        let Some(node) = string_arg(vm, node) else { return script_err_invalid_args!(vm.trap(), "open(node): the graphic's clip, a string") };
        OPENS.with(|o| o.borrow_mut().push(node));
        NIL
    });
}

/// Register the document kind the panels edit (`doc`) and what every panel isolate gets: the panel widgets' library
/// (`makepad_value_pickers::panel`), a host's own widget libraries
/// (`more`, each making the isolate's prelude again, as the synth widgets
/// do), then `scene`; and the [`EditorPanel`] widget. Once, after
/// `makepad_widgets::script_mod`.
pub fn register(vm: &mut ScriptVm, doc: Rc<dyn PanelDoc>, more: &[fn(&mut ScriptVm)]) {
    install_doc(doc);
    ISOLATE_MODS.with(|m| {
        let mut m = m.borrow_mut();
        m.push(makepad_value_pickers::panel::isolate_mod);
        m.extend_from_slice(more);
        m.push(scene_module);
    });
    makepad_widgets::widget_async::register_splash_isolate_mod(isolate_mods);
    script_mod(vm);
}

/// Every panel isolate's libraries, in order; a library whose script fails
/// in the isolate says so in the log (the panel would only say a widget is
/// not found).
fn isolate_mods(vm: &mut ScriptVm) {
    let mods: Vec<fn(&mut ScriptVm)> = ISOLATE_MODS.with(|m| m.borrow().clone());
    for install in mods {
        vm.capture_errors_nested();
        install(vm);
        let errors = vm.take_errors();
        vm.capture_errors_restore();
        if !errors.is_empty() {
            log!("editor panel: a widget library failed in the panel's isolate: {}", errors.join("; "));
        }
    }
}

/// The panel widgets' host (`value_pickers::panel::host`) over this
/// panel: they read what `scene.read` reads and set through the same
/// gesture rules as `scene.set`. `more` adds what the host has beyond
/// paths (`fns_like`, a palette, …). The host whose document is open
/// installs it when it mounts its panel.
pub fn install_host(more: impl FnOnce(PanelHost) -> PanelHost) {
    use makepad_value_pickers::panel::host as panel_host;
    let host = PanelHost {
        swap: Some(Box::new(|path: &str, name: &str| queue(Edit::Swap { path: path.to_string(), name: name.to_string() }))),
        fns_like: Some(Box::new(|path: &str| SCENE.with(|s| fns_like(&s.borrow().files(), path)))),
        // The files' colours, each with how many places write it.
        palette: Some(Box::new(|_scope: &str| SCENE.with(|s| {
            let doc = doc();
            let mut out: Vec<panel_host::Swatch> = Vec::new();
            for (name, text, code) in &s.borrow().files {
                for found in doc.palette(text) {
                    let refer = found.sites.iter().find_map(|site| doc.let_of_literal(text, &**code, site)).map(|n| if name.is_empty() { n } else { format!("{name}.{n}") });
                    match out.iter_mut().find(|w| w.rgba == found.rgba) {
                        Some(w) => {
                            w.uses += found.sites.len();
                            if w.refer.is_none() {
                                w.refer = refer;
                            }
                        }
                        None => out.push(panel_host::Swatch { rgba: found.rgba, name: found.name.unwrap_or_default(), uses: found.sites.len(), refer }),
                    }
                }
            }
            // A gesture holding a colour where one is written: that colour
            // as it is now (the palette follows the drag).
            let scene = s.borrow();
            let files = scene.files();
            HELD.with(|h| {
                for (path, text) in h.borrow().iter() {
                    let Ok(site) = doc.values(&files).resolve(path) else { continue };
                    let (Some(was), Some(now)) = (doc.palette(&site.text).first().map(|w| w.rgba), doc.palette(text).first().map(|w| w.rgba)) else { continue };
                    if let Some(w) = out.iter_mut().find(|w| w.rgba == was) {
                        w.rgba = now;
                    }
                }
            });
            out
        }))),
        recolour: Some(Box::new(|_scope: &str, map| queue(Edit::Recolour { map }))),
        // A control's own press and release (the structure column's
        // controls are outside the panel's area): its gesture held.
        hold: Some(Box::new(|on: bool| if on { press() } else { release() })),
        ..PanelHost::new(
            widget_read,
            |path, value| {
                queue(match value {
                    PanelValue::Number(n) => Edit::Set { path: path.to_string(), value: Json::F64(n) },
                    PanelValue::Bool(b) => Edit::Set { path: path.to_string(), value: Json::Bool(b) },
                    PanelValue::Code(code) => Edit::SetCode { path: path.to_string(), code },
                })
            },
        )
    };
    makepad_value_pickers::panel::host::install(more(host));
}

/// The document's code changed: bound widgets read their paths again.
pub fn changed() {
    makepad_value_pickers::panel::host::changed();
}

/// The functions that could stand where the function a path names is: the
/// same parameters, in the document or a module it imports.
pub fn fns_like(files: &[File], path: &str) -> Vec<String> {
    doc().fns_like(files, path)
}

/// The bytes of `old` that `new` changes, and where that run ends in `new`.
fn common_span(old: &str, new: &str) -> (Range<usize>, usize) {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(start) {
        start -= 1;
    }
    let mut tail = a[start..].iter().rev().zip(b[start..].iter().rev()).take_while(|(x, y)| x == y).count();
    while !old.is_char_boundary(a.len() - tail) || !new.is_char_boundary(b.len() - tail) {
        tail -= 1;
    }
    (start..a.len() - tail, b.len() - tail)
}

/// A host's own bound controls (a View holding panel widgets outside an
/// [`EditorPanel`]): a press in it is a gesture as in a panel.
#[derive(Script, ScriptHook, Widget)]
pub struct PanelHand {
    #[deref]
    view: View,
    #[rust]
    pressed: bool,
}

impl Widget for PanelHand {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event {
            Event::MouseDown(e) if self.view.area().rect(cx).contains(e.abs) => {
                self.pressed = true;
                press();
            }
            Event::MouseUp(_) if std::mem::take(&mut self.pressed) => release(),
            _ => {}
        }
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}

#[derive(Clone, Debug, Default)]
pub enum EditorPanelAction {
    #[default]
    None,
    /// "Make me an editor" (no panel yet).
    Make,
    /// A hand moved in the panel: its edits (or its widgets' asks) wait
    /// for the host's next turn.
    Touched,
}

#[derive(Script, ScriptHook, Widget)]
pub struct EditorPanel {
    #[deref]
    view: View,
}

impl Widget for EditorPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The program draws in its own pass: a module it asked to show is
        // brought into view on the next event (its next frame).
        if let Event::NextFrame(_) = event {
            self.scroll_to_asked(cx);
        }
        // A gesture in the panel: its edits are held until the release.
        match event {
            Event::MouseDown(e) if self.view.area().rect(cx).contains(e.abs) => PRESSED.with(|p| p.set(true)),
            Event::MouseUp(_) if PRESSED.with(|p| p.get()) => {
                PRESSED.with(|p| p.set(false));
                RELEASED.with(|r| r.set(true));
            }
            _ => {}
        }
        let actions = makepad_value_pickers::panel::host::timed("panel.event", || cx.capture_actions(|cx| self.view.handle_event(cx, event, scope)));
        if self.view.button(cx, ids!(p_make)).clicked(&actions) {
            cx.widget_action(self.widget_uid(), EditorPanelAction::Make);
        }
        cx.extend_actions(actions);
        let over = |abs: DVec2| self.view.area().rect(cx).contains(abs);
        let touched = match event {
            Event::MouseDown(e) => over(e.abs),
            Event::MouseMove(e) => over(e.abs) || PRESSED.with(|p| p.get()),
            Event::MouseUp(_) => true,
            Event::Scroll(e) => over(e.abs),
            // Edits a program made on its own (a timer, a widget's
            // animation); never on the actions this sends, or it would
            // answer itself until the host takes the edits (every panel
            // of the app shares the queue).
            Event::Actions(_) => false,
            _ => EDITS.with(|e| !e.borrow().is_empty()),
        };
        if touched {
            cx.widget_action(self.widget_uid(), EditorPanelAction::Touched);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        makepad_value_pickers::panel::host::timed("panel.draw", || while !self.view.draw_walk(cx, scope, walk).is_done() {});
        self.scroll_to_asked(cx);
        self.show_opened(cx);
        DrawStep::done()
    }
}

impl EditorPanel {
    /// Something opened by hand (a module, a group, a picker) whose new
    /// content runs past the bottom: scrolled up smoothly until it shows,
    /// never so far that its heading leaves the top.
    fn show_opened(&mut self, cx: &mut Cx) {
        let Some(rect) = makepad_value_pickers::panel::host::take_opened() else { return };
        let scroll = self.view.view(cx, ids!(p_scroll));
        let visible = scroll.area().rect(cx).size.y;
        if visible <= 0.0 {
            return;
        }
        let height = (rect.size.y + 8.0).min(visible - 8.0).max(0.0);
        if let Some(mut view) = scroll.borrow_mut() {
            view.scroll_into_view_abs(cx, Rect { pos: rect.pos, size: dvec2(rect.size.x, height) });
        }
        scroll.redraw(cx);
    }

    /// A module the panel opened for someone (the index, a selection) comes
    /// into view: its top near the top when it is above the view or in its
    /// lower half (it opens downwards).
    fn scroll_to_asked(&mut self, cx: &mut Cx) {
        let Some(rect) = makepad_value_pickers::panel::host::take_scroll_to() else { return };
        let scroll = self.view.view(cx, ids!(p_scroll));
        let shown = scroll.area().rect(cx);
        if let Some(extent) = scroll.scroll_extent() {
            if rect.pos.y < shown.pos.y || rect.pos.y > shown.pos.y + shown.size.y * 0.5 {
                // Not clamped to the content as it is: the module opens on
                // the frame this one draws, and the content grows with it.
                let y = (extent.pos.y + rect.pos.y - shown.pos.y - 8.0).max(0.0);
                if let Some(mut view) = scroll.borrow_mut() {
                    view.set_scroll_pos_unclipped(cx, dvec2(extent.pos.x, y));
                }
                self.view.redraw(cx);
            }
        }
    }
}

impl EditorPanelRef {
    /// "Make me an editor" was clicked.
    pub fn make_clicked(&self, actions: &Actions) -> bool {
        actions.filter_widget_actions_cast::<EditorPanelAction>(self.widget_uid()).any(|a| matches!(a, EditorPanelAction::Make))
    }

    /// A hand moved in the panel (the host takes its edits on its next turn).
    pub fn touched(&self, actions: &Actions) -> bool {
        actions.filter_widget_actions_cast::<EditorPanelAction>(self.widget_uid()).any(|a| matches!(a, EditorPanelAction::Touched))
    }
}

// ------------------------------------------------------------------ the host's side

/// The top-level `let editor_panel` of a document: its bytes.
pub fn panel_region(text: &str, code: &dyn Any) -> Option<Range<usize>> {
    let_region(text, code, "editor_panel")
}

/// The top-level `let <name>` of a document: its bytes.
pub fn let_region(text: &str, code: &dyn Any, name: &str) -> Option<Range<usize>> {
    doc().let_region(text, code, name)
}

/// The panel's program as written: the body of the View `editor_panel`
/// returns.
pub fn body_of(source: &str, code: &dyn Any) -> Option<String> {
    body_of_let(source, code, "editor_panel")
}

/// The body of the View the top-level `let <name> = fn() { View{…} }`
/// returns (an edit's panel is `cut_panel`).
pub fn body_of_let(source: &str, code: &dyn Any, name: &str) -> Option<String> {
    let text = &source[let_region(source, code, name)?];
    let open = text.find("View{").or_else(|| text.find("View {"))?;
    let brace = open + text[open..].find('{')?;
    let bytes = text.as_bytes();
    let (mut depth, mut in_string) = (0i32, false);
    for i in brace..bytes.len() {
        match bytes[i] {
            b'"' if i == 0 || bytes[i - 1] != b'\\' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[brace + 1..i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// `source` with its `let editor_panel` replaced by `code` (the whole
/// `let editor_panel = fn() { View{…} }`), or `code` added before the line
/// starting with `anchor` (`Motion{`, `Score{`) when it has none.
pub fn with_panel(source: &str, code: &str, anchor: &str) -> Result<String, String> {
    with_let(source, code, anchor, "editor_panel")
}

/// [`with_panel`] for the panel `let <name>`.
pub fn with_let(source: &str, code: &str, anchor: &str, name: &str) -> Result<String, String> {
    let code = code.trim();
    if !code.starts_with(&format!("let {name}")) {
        return Err(format!("the code is the whole `let {name} = fn() {{ View{{…}} }}`"));
    }
    let outline = doc().outline(source);
    Ok(match let_region(source, &*outline, name) {
        Some(region) => format!("{}{}{}", &source[..region.start], code, &source[region.end..]),
        None => {
            let at = source.find(&format!("\n{anchor}")).map(|i| i + 1).ok_or_else(|| format!("the document has no `{anchor}` at the start of a line"))?;
            format!("{}{code}\n\n{}", &source[..at], &source[at..])
        }
    })
}

/// Every path a panel can bind, a line each: `path (kind reach) = value
/// // label`. Records, lists and code are left out unless tagged
/// (`/**name*/`). `reach` says what a held value does there (the host
/// knows: `RELOAD` on the graphic desk for a value read while the scene
/// loads; on SCORE `live` or `on release`, and the range).
pub fn listing(files: &[File], reach: &dyn Fn(&ValueSite) -> Option<String>) -> String {
    let mut out = String::new();
    let mut labelled = std::collections::HashSet::new();
    let doc = doc();
    for site in doc.values(files).sites() {
        if site.id.starts_with("editor_panel") {
            continue;
        }
        let tagged = site.label.starts_with("tagged");
        if matches!(site.kind, ValueKind::Code | ValueKind::List | ValueKind::Record) && !tagged {
            continue;
        }
        let value: String = site.text.chars().take(48).collect();
        let root = site.id.split(['.', '[']).next().unwrap_or("").to_string();
        let how = reach(&site).map(|r| format!(" {r}")).unwrap_or_default();
        let label = Some(&site.label).filter(|l| !l.is_empty() && labelled.insert(root)).map(|l| format!("  // {}", l.chars().take(90).collect::<String>())).unwrap_or_default();
        out.push_str(&format!("{} ({}{how}) = {value}{label}\n", site.id, site.kind.name()));
    }
    out
}

/// What an edit's panel can bind, a line each: `path (kind[, n places])
/// = value  // label` ([`PanelDoc::list_in`]: the clips'
/// times, the shared modules' paths, the paths several graphics share as
/// `*:PATH`). Records, lists and code are left out.
pub fn listing_docs(files: &[File], table: &dyn Any) -> String {
    let mut out = String::new();
    let mut labelled = std::collections::HashSet::new();
    for (site, reach) in doc().list_in(files, table) {
        if matches!(site.kind, ValueKind::Code | ValueKind::Record) && !site.id.ends_with(".span") || site.id.contains("editor_panel") {
            continue;
        }
        let value: String = site.text.chars().take(48).collect();
        let root = site.id.split(['.', '[']).next().unwrap_or("").to_string();
        let reach = if reach > 1 { format!(", {reach} places") } else { String::new() };
        let label = Some(&site.label).filter(|l| !l.is_empty() && labelled.insert(root)).map(|l| format!("  // {}", l.chars().take(90).collect::<String>())).unwrap_or_default();
        out.push_str(&format!("{} ({}{reach}) = {value}{label}\n", site.id, site.kind.name()));
    }
    out
}

/// A host's panel: what is mounted, the gesture in flight, the state line.
#[derive(Default)]
pub struct Runtime {
    mounted: Option<Option<String>>,
    gesture: u64,
    /// The gesture's last edit per path (written at the release, the path
    /// resolved again then: a host may or may not have spliced the steps).
    held: Vec<(String, Edit)>,
    /// A set's members' numbers where the gesture began, per path: each
    /// moves by as much as the control moved from the first.
    origins: Vec<(String, Vec<f64>)>,
    /// A recolour gesture over several documents: their texts as it began,
    /// and whether one is held.
    recolour_base: Option<Vec<(String, Vec<(Range<usize>, [f64; 4])>)>>,
    recolour_held: bool,
    in_gesture: bool,
    pub status: String,
    status_shown: Option<String>,
}

impl Runtime {
    /// Mount the program (`None`: the document has none) when it changed.
    /// True when it changed.
    pub fn mount(&mut self, cx: &mut Cx, panel: &WidgetRef, body: Option<String>) -> bool {
        if self.mounted.as_ref() == Some(&body) {
            self.show_status(cx, panel);
            return false;
        }
        self.mounted = Some(body.clone());
        // (The old program's places go with it.)
        makepad_value_pickers::panel::host::program_changed();
        let splash = panel.widget(cx, ids!(p_splash));
        match &body {
            Some(body) => {
                // Mounted either way (what runs of it shows), the mount's own
                // errors in the state line: no second isolate to check it first.
                splash.as_splash().set_text(cx, body);
                let errors = splash.as_splash().last_errors();
                self.status = match errors.first() {
                    Some(error) => {
                        log!("editor panel: {}", errors.join("; "));
                        format!("The panel has errors: {error}")
                    }
                    None => String::new(),
                };
            }
            None => {
                splash.as_splash().set_text(cx, "");
                self.status.clear();
            }
        }
        panel.widget(cx, ids!(p_scroll)).set_visible(cx, body.is_some());
        panel.widget(cx, ids!(p_empty)).set_visible(cx, body.is_none());
        self.show_status(cx, panel);
        panel.redraw(cx);
        true
    }

    /// Forget what is mounted (the document closed).
    pub fn unmount(&mut self) {
        self.mounted = None;
        self.held.clear();
        self.origins.clear();
        HELD.with(|h| h.borrow_mut().clear());
        self.in_gesture = false;
    }

    /// Whether a program is mounted (`None`: nothing mounted yet).
    /// The program mounted now (None: none).
    pub fn mounted_body(&self) -> Option<String> {
        self.mounted.clone().flatten()
    }

    pub fn mounted(&self) -> Option<bool> {
        self.mounted.as_ref().map(|body| body.is_some())
    }

    /// The program's widgets draw again (they read the held values: a
    /// knob shows what another control under the hand moves).
    pub fn redraw_program(&self, cx: &mut Cx, panel: &WidgetRef) {
        panel.widget(cx, ids!(p_splash)).redraw(cx);
    }

    /// A gesture is held (its values not written yet).
    pub fn holding(&self) -> bool {
        !self.held.is_empty() || self.recolour_held
    }

    fn show_status(&mut self, cx: &mut Cx, panel: &WidgetRef) {
        if self.status_shown.as_ref() != Some(&self.status) {
            self.status_shown = Some(self.status.clone());
            panel.widget(cx, ids!(p_status)).set_text(cx, &self.status);
            panel.widget(cx, ids!(p_status)).set_visible(cx, !self.status.is_empty());
        }
    }

    /// The edits the program asked for since the last turn, as splices on
    /// `files` (the host's, in the order it gave [`set_files`] or
    /// [`set_docs`]). Steps of a gesture come back `done: false`; the
    /// release re-sends the last step of each site `done: true` (one undo
    /// step). A set's path comes back as a splice per member. A path that
    /// names nothing written yet goes to `missing(path, text)`: the host
    /// may say where it would be written (a field added to its record), as
    /// (file, range, text).
    pub fn take(&mut self, files: &[File], missing: &dyn Fn(&str, &str) -> Option<(usize, Range<usize>, String)>) -> Vec<PanelEdit> {
        let edits: Vec<Edit> = EDITS.with(|e| std::mem::take(&mut *e.borrow_mut()));
        let pressed = PRESSED.with(|p| p.get());
        let released = RELEASED.with(|r| r.replace(false));
        let mut out = Vec::new();
        if edits.is_empty() && !released {
            return out;
        }
        if pressed && !self.in_gesture && !edits.is_empty() {
            self.in_gesture = true;
            self.gesture += 1;
        }
        let gesture = u64::MAX / 2 + self.gesture;
        for edit in edits {
            let (path, live) = match &edit {
                Edit::Set { path, .. } | Edit::SetCode { path, .. } => (path.clone(), pressed),
                Edit::Swap { path, .. } | Edit::Replace { path, .. } => (path.clone(), false),
                Edit::Recolour { map } if SCENE.with(|s| s.borrow().table.is_some()) => {
                    let doc = doc();
                    // Over several documents a recolour is a gesture like a
                    // set: each step moves the colours as they were when it
                    // began (held), the release writes the last.
                    // (Each file's colours read once for the gesture.)
                    let base = self.recolour_base.get_or_insert_with(|| files.iter().map(|f| (f.text.to_string(), doc.palette_sites(f.text))).collect());
                    let live = pressed;
                    let mut any = false;
                    for (k, f) in files.iter().enumerate() {
                        let Some((was, sites)) = base.get(k) else { continue };
                        let next = doc.recolour_at(was, sites, map);
                        if next != f.text {
                            let (a, b) = common_span(f.text, &next);
                            out.push(PanelEdit { splice: Splice { file: k, range: a.clone(), text: next[a.start..b].to_string() }, gesture, done: !live, path: String::new(), kind: ValueKind::Colour });
                            any = true;
                        }
                    }
                    if !live || !any {
                        self.recolour_base = None;
                    }
                    if live {
                        self.recolour_held = true;
                    }
                    continue;
                }
                Edit::Recolour { map } => {
                    // Every place each colour is written, in its form.
                    let doc = doc();
                    for (k, f) in files.iter().enumerate() {
                        let next = doc.recolour(f.text, map);
                        if next != f.text {
                            let (a, b) = common_span(f.text, &next);
                            out.push(PanelEdit { splice: Splice { file: k, range: a.clone(), text: next[a.start..b].to_string() }, gesture, done: true, path: String::new(), kind: ValueKind::Colour });
                        }
                    }
                    continue;
                }
            };
            let bounds = match bounds(files, &path) {
                Ok(bounds) => bounds,
                Err(why) => {
                    let text = edit_text(&edit);
                    match missing(&path, &text) {
                        Some((file, range, written)) => {
                            if live {
                                self.hold(&path, &edit, text);
                            }
                            out.push(PanelEdit { splice: Splice { file, range, text: written }, gesture, done: !live, path, kind: ValueKind::Code });
                        }
                        None => self.status = why,
                    }
                    continue;
                }
            };
            let texts = match self.texts(files, &path, &bounds, &edit, live) {
                Ok(texts) => texts,
                Err(why) => {
                    self.status = why;
                    continue;
                }
            };
            // A value written outright supersedes the gesture's held one;
            // the panel's other widgets show the held value too.
            self.held.retain(|(p, _)| *p != path);
            if live {
                self.hold(&path, &edit, texts[0].1.clone());
            } else {
                HELD.with(|h| h.borrow_mut().retain(|(p, _)| *p != path));
                changed();
            }
            for (bound, (range, text)) in bounds.into_iter().zip(texts) {
                out.push(PanelEdit { splice: Splice { file: bound.site.file(), range, text }, gesture, done: !live, path: path.clone(), kind: bound.site.kind });
            }
        }
        if released {
            self.in_gesture = false;
            // A held recolour ends: its last step is in the files already
            // (the host writes what they hold now that nothing is held).
            self.recolour_held = false;
            self.recolour_base = None;
            HELD.with(|h| h.borrow_mut().clear());
            for (path, edit) in std::mem::take(&mut self.held) {
                match bounds(files, &path) {
                    Ok(bounds) => match self.texts(files, &path, &bounds, &edit, false) {
                        Ok(texts) => {
                            for (bound, (range, text)) in bounds.into_iter().zip(texts) {
                                out.push(PanelEdit { splice: Splice { file: bound.site.file(), range, text }, gesture, done: true, path: path.clone(), kind: bound.site.kind });
                            }
                        }
                        Err(why) => self.status = why,
                    },
                    Err(why) => match missing(&path, &edit_text(&edit)) {
                        Some((file, range, written)) => out.push(PanelEdit { splice: Splice { file, range, text: written }, gesture, done: true, path, kind: ValueKind::Code }),
                        None => self.status = why,
                    },
                }
            }
        }
        if !self.in_gesture {
            self.origins.clear();
        }
        // One splice per site (the last asked; done if any was), applied in
        // turn from the end of each file back: every range is in the text
        // the host gave, and none moves under another.
        let mut merged: Vec<PanelEdit> = Vec::new();
        for edit in out {
            match merged.iter_mut().find(|m| m.splice.file == edit.splice.file && m.splice.range.start == edit.splice.range.start) {
                Some(m) => {
                    let done = m.done || edit.done;
                    *m = PanelEdit { done, ..edit };
                }
                None => merged.push(edit),
            }
        }
        merged.sort_by(|a, b| a.splice.file.cmp(&b.splice.file).then(b.splice.range.start.cmp(&a.splice.range.start)));
        merged
    }

    /// The gesture holds `edit` at `path`; the program reads `text` there.
    fn hold(&mut self, path: &str, edit: &Edit, text: String) {
        self.held.retain(|(p, _)| p != path);
        self.held.push((path.to_string(), edit.clone()));
        HELD.with(|h| {
            let mut h = h.borrow_mut();
            h.retain(|(p, _)| p != path);
            h.push((path.to_string(), text));
        });
        changed();
    }

    /// What `edit` writes at each of `bounds`, through the one edit
    /// primitive ([`ValueSource::write`]): the literal token there and the
    /// value in its form. A number set on a set (or a roll) moves each
    /// member from where the gesture began by as much as the first moved (a
    /// clip's time by whole beats).
    /// A held step (`live`) is placed as the write would be, without the
    /// write's read-back: it is not written into the text; its release is.
    fn texts(&mut self, files: &[File], path: &str, bounds: &[Bound], edit: &Edit, live: bool) -> Result<Vec<(Range<usize>, String)>, String> {
        let doc = doc();
        let number = match edit {
            Edit::Set { value: Json::Int(n), .. } => Some(*n as f64),
            Edit::Set { value: Json::F64(n), .. } => Some(*n),
            _ => None,
        };
        let numeric = bounds.iter().all(|b| b.site.is_numeric());
        let snap = |from: f64, to: f64| SCENE.with(|s| s.borrow().table.as_ref().map_or(to, |t| doc.snap(&**t, path, from, to)));
        let values: Vec<Value> = match (edit, number) {
            (Edit::Set { .. }, Some(v)) if numeric && (bounds.len() > 1 || bounds.iter().any(|b| b.scale != 1.0) || doc.snaps(path)) => {
                let origin = match self.origins.iter().find(|(p, _)| p == path) {
                    Some((_, o)) if o.len() == bounds.len() => o.clone(),
                    _ => {
                        let o: Vec<f64> = bounds.iter().map(|b| match read_site(&b.site) {
                            Read::Number(n) => n,
                            _ => 0.0,
                        }).collect();
                        self.origins.retain(|(p, _)| p != path);
                        self.origins.push((path.to_string(), o.clone()));
                        o
                    }
                };
                // Each member moves as the first did; a clip's time lands on
                // the song's nearest beat (each member's own: its first place
                // is where it is, a cut or a start).
                let d = v - origin[0];
                let mut moved: Vec<(&str, f64)> = Vec::new();
                origin.iter().zip(bounds).map(|(o, b)| {
                    let dm = match moved.iter().find(|(m, _)| *m == b.member) {
                        Some((_, dm)) => *dm,
                        None => {
                            let dm = snap(*o, o + d) - o;
                            moved.push((&b.member, dm));
                            dm
                        }
                    };
                    Value::Num(((o + b.scale * dm) * 1e4).round() / 1e4)
                }).collect()
            }
            (Edit::Set { value, .. }, _) => bounds.iter().map(|b| match value {
                Json::Int(n) => Ok(Value::Num(*n as f64)),
                Json::F64(n) => Ok(Value::Num(*n)),
                Json::Bool(v) => Ok(Value::Bool(*v)),
                Json::Str(t) if b.site.kind == ValueKind::Text => Ok(Value::Text(t.clone())),
                Json::Str(t) => Ok(Value::Code(t.clone())),
                _ => Err(format!("{} cannot take that value", b.site.kind.name())),
            }).collect::<Result<_, _>>()?,
            (Edit::SetCode { code, .. } | Edit::Replace { code, .. }, _) => vec![Value::Code(code.clone()); bounds.len()],
            (Edit::Swap { name, .. }, _) => vec![Value::Code(name.clone()); bounds.len()],
            (Edit::Recolour { .. }, _) => return Ok(Vec::new()),
        };
        // A keyed value (`keys(…)`): a number set on it is its key at the
        // playhead (that key's value, or a new key there), never the
        // animation replaced by a constant.
        if bounds.len() == 1 && bounds[0].site.text.trim_start().starts_with("keys(") {
            let number = match &values[0] {
                Value::Num(v) => Some(*v),
                Value::Code(c) => c.trim().parse::<f64>().ok(),
                _ => None,
            };
            if let Some(v) = number {
                let v = &v;
                let site = &bounds[0].site;
                let text = files[site.file()].text;
                let t = makepad_value_pickers::panel::host::playhead().map_or(0.0, |p| p.0);
                let half = 0.5 / 60.0;
                let v = (v * 1e4).round() / 1e4;
                return doc.keys_set(text, site.range(), t, half, v).map(|set| vec![set]).ok_or_else(|| format!("`{path}`: its keys do not read"));
            }
        }
        // One document: the path written and read back. Several: each
        // member's literal token, in its form.
        if SCENE.with(|s| s.borrow().table.is_none()) && bounds.len() == 1 {
            if live {
                return Ok(vec![doc.place(files, path, &values[0])?]);
            }
            let written = doc.values(files).write(path, &values[0])?;
            return written.into_iter().next().map(|s| vec![(s.range, s.text)]).ok_or_else(|| format!("`{path}`: nothing to write"));
        }
        bounds.iter().zip(&values).map(|(b, value)| {
            let mut site = b.site.clone();
            let (file, mut range) = (site.file(), site.range());
            if !matches!(value, Value::Code(_)) {
                range = doc.literal_token(files[file].text, &range).ok_or_else(|| format!("`{path}` is not one literal (`{}`)", site.text.trim()))?;
                site.text = files[file].text[range.clone()].to_string();
                site.span = Some((file, range.clone()));
            }
            Ok((range, doc.form(&site, value)?))
        }).collect()
    }
}

/// An edit's value as code, for a path that names nothing written yet.
fn edit_text(edit: &Edit) -> String {
    match edit {
        Edit::Set { value: Json::Int(n), .. } => fmt_num(*n as f64),
        Edit::Set { value: Json::F64(n), .. } => fmt_num(*n),
        Edit::Set { value: Json::Bool(b), .. } => b.to_string(),
        Edit::Set { value: Json::Str(t), .. } => fmt_text(t),
        Edit::SetCode { code, .. } | Edit::Replace { code, .. } => code.clone(),
        Edit::Swap { name, .. } => name.clone(),
        Edit::Set { .. } | Edit::Recolour { .. } => String::new(),
    }
}
