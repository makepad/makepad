//! What a panel widget is bound to: a path into the scene's code
//! (`WORD_SPARKS.count`, `LYRIC_LOOK.font`, `camera_keys[3].zoom`) that
//! the host reads and writes.
//!
//! The host (the desk that mounts the scene's editor panel) installs a
//! [`PanelHost`] and calls [`changed`] whenever the scene's code changes (a
//! write, an undo, a load); each bound widget reads its path again on its
//! next draw. A widget's change calls [`set`] at every step of a gesture:
//! the host holds the steps live while a finger is down in the panel and
//! writes the release as one undo step, as it does for the panel program's
//! own `scene.set`.

use makepad_widgets::{Area, Cx, Cx2d, DVec2, Rect};
use std::cell::{Cell, RefCell};

/// A value as the code writes it.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelValue {
    Number(f64),
    Bool(bool),
    /// Anything else, as written: `#rrggbb`, `color(3, 2, 1.2)`, `@name`,
    /// `Font{family: "Archivo" weight: 700}`, `"words"`, `[0.1, 0.2]`,
    /// `vec2(0.5, 1)`, `0.12s`, `90deg`, `cubic(0.2, 0, 0.1, 1)`.
    Code(String),
}

impl PanelValue {
    /// The value as code text.
    pub fn code(&self) -> String {
        match self {
            Self::Number(v) => fmt_number(*v, 4),
            Self::Bool(b) => b.to_string(),
            Self::Code(c) => c.clone(),
        }
    }

    /// The value as a number and its unit (`0.12s` → 0.12, "s").
    pub fn number(&self) -> Option<(f64, String)> {
        match self {
            Self::Number(v) => Some((*v, String::new())),
            Self::Bool(b) => Some((if *b { 1.0 } else { 0.0 }, String::new())),
            Self::Code(c) => number_with_unit(c),
        }
    }

    /// Every number the value writes, in order (`vec2(0.5, 1)` → [0.5, 1]).
    pub fn numbers(&self) -> Vec<f64> {
        match self {
            Self::Number(v) => vec![*v],
            Self::Bool(b) => vec![if *b { 1.0 } else { 0.0 }],
            Self::Code(c) => numbers_in(c),
        }
    }
}

/// `v` written with at most `decimals` decimals and no trailing zeros.
pub fn fmt_number(v: f64, decimals: usize) -> String {
    let mut s = format!("{:.*}", decimals, v);
    while s.contains('.') && (s.ends_with('0') || s.ends_with('.')) {
        s.pop();
    }
    if s == "-0" { "0".into() } else { s }
}

/// A number written with a unit: `0.12s`, `120ms`, `90deg`, `0.3rad`, `12px`.
pub fn number_with_unit(text: &str) -> Option<(f64, String)> {
    let t = text.trim();
    ["ms", "deg", "rad", "px", "s", ""].into_iter().find_map(|unit| {
        let digits = t.strip_suffix(unit)?;
        if !unit.is_empty() && !digits.ends_with(|c: char| c.is_ascii_digit() || c == '.') {
            return None;
        }
        digits.parse::<f64>().ok().map(|v| (v, unit.to_string()))
    })
}

/// The numbers written in a value (`vec2(0.5, -1)`, `[1, 2, 3]`), as they
/// appear.
pub fn numbers_in(text: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let starts = b[i].is_ascii_digit() || (b[i] == b'-' && b.get(i + 1).is_some_and(|c| c.is_ascii_digit() || *c == b'.')) || (b[i] == b'.' && b.get(i + 1).is_some_and(|c| c.is_ascii_digit()));
        // Not the digits of a name (`vec2`) or a hex colour.
        let in_word = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'#');
        if starts && !in_word {
            let s = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || b[i] == b'e' || (b[i] == b'-' && b[i - 1] == b'e')) {
                i += 1;
            }
            if let Ok(v) = text[s..i].parse::<f64>() {
                out.push(v);
            }
            continue;
        }
        i += 1;
    }
    out
}

/// `text` with its numbers replaced by `values`, in order (a value's form
/// kept: `vec2(0.5, 1)` stays a `vec2`).
pub fn with_numbers(text: &str, values: &[f64], decimals: usize) -> String {
    let b = text.as_bytes();
    let mut out = String::new();
    let mut k = 0;
    let mut i = 0;
    let mut last = 0;
    while i < b.len() {
        let starts = b[i].is_ascii_digit() || (b[i] == b'-' && b.get(i + 1).is_some_and(|c| c.is_ascii_digit() || *c == b'.')) || (b[i] == b'.' && b.get(i + 1).is_some_and(|c| c.is_ascii_digit()));
        let in_word = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'#');
        if starts && !in_word {
            let s = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || b[i] == b'e' || (b[i] == b'-' && b[i - 1] == b'e')) {
                i += 1;
            }
            if let Some(v) = values.get(k) {
                out.push_str(&text[last..s]);
                out.push_str(&fmt_number(*v, decimals));
                last = i;
            }
            k += 1;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    out
}

/// A colour of the scene's palette: its value (channels over 1 for light),
/// its name (its tag, or the `let` it is) and how many places use it.
#[derive(Clone, Debug, PartialEq)]
pub struct Swatch {
    pub rgba: [f64; 4],
    pub name: String,
    pub uses: usize,
    /// The name code refers to it by, when a `let` is just this colour
    /// (`BONE`, `look.BONE` for a module's): what a control written as a
    /// name writes when it is picked.
    pub refer: Option<String>,
}

type Read = Box<dyn Fn(&str) -> Option<PanelValue>>;
type Set = Box<dyn Fn(&str, PanelValue)>;

/// What the host gives the panel's widgets: `read` and `set` (paths), and
/// what the larger widgets need when the host has it.
pub struct PanelHost {
    /// The value at a path now (`None`: the path does not resolve).
    pub read: Read,
    /// A new value at a path (each step of a gesture; the host holds it
    /// while a finger is down in the panel and writes the release as one
    /// undo step).
    pub set: Set,
    /// A function slot names another function.
    pub swap: Option<Box<dyn Fn(&str, &str)>>,
    /// The functions that could stand where a path's function is (the same
    /// parameters).
    pub fns_like: Option<Box<dyn Fn(&str) -> Vec<String>>>,
    /// The palette of a scope: a file's name, a tag, or "" for the scene.
    pub palette: Option<Box<dyn Fn(&str) -> Vec<Swatch>>>,
    /// Colours of a scope moved, old to new, at every place each is written
    /// (held like `set`, written once at the release).
    pub recolour: Option<Box<dyn Fn(&str, Vec<([f64; 4], [f64; 4])>)>>,
    /// The `n` colours the frame at the playhead is mostly made of.
    pub frame_colours: Option<Box<dyn Fn(usize) -> Vec<[f64; 4]>>>,
    /// Saved palettes: each name and its colours.
    pub saved_palettes: Option<Box<dyn Fn() -> Vec<(String, Vec<[f64; 4]>)>>>,
    /// A path's value in the running film at the playhead, as evaluated
    /// (what a meter shows, what a modulated knob's ring reaches).
    pub live: Option<Box<dyn Fn(&str) -> Option<f64>>>,
    /// The playhead: its time, the film's length, and whether it plays.
    pub playhead: Option<Box<dyn Fn() -> (f64, f64, bool)>>,
    /// The song's loudness over `t0..t1` in `n` peaks (0..1).
    pub song: Option<Box<dyn Fn(f64, f64, usize) -> Vec<f32>>>,
    /// The beats' times over `t0..t1`.
    pub beats: Option<Box<dyn Fn(f64, f64) -> Vec<f64>>>,
    /// The sung words over `t0..t1`: each start, end and words.
    pub words: Option<Box<dyn Fn(f64, f64) -> Vec<(f64, f64, String)>>>,
    /// Ask the scene's agent to change what `targets` (paths) say:
    /// `mode` "replace", "variants" or "extend". The ask's id.
    pub ask: Option<Box<dyn Fn(&str, &[String], &str) -> Option<u64>>>,
    /// An ask's progress: "working", "done: …" or "failed: …".
    pub ask_status: Option<Box<dyn Fn(u64) -> String>>,
    /// The paths the person is on in the panel: a control's (or a module's,
    /// every path in it) while the pointer is over it, a selected module's
    /// until another click or Escape; `None` when on nothing.
    pub focus: Option<Box<dyn Fn(Option<&[String]>)>>,
    /// Whether a path matters now: `Some(true)` the playhead is in its
    /// spans, `Some(false)` not, `None` not known (drawn as usual). The
    /// picture's selection does not change it (see `picked`).
    pub active: Option<Box<dyn Fn(&str) -> Option<bool>>>,
    /// Whether the picture's selection reads a path (its modules and
    /// controls are marked, the rest drawn as usual).
    pub picked: Option<Box<dyn Fn(&str) -> bool>>,
    /// Rises whenever an `active` answer may have changed.
    pub lit_rev: Option<Box<dyn Fn() -> u64>>,
    /// Rises when the picture's selection changes (the panel shows the
    /// first active module).
    pub reveal: Option<Box<dyn Fn() -> u64>>,
    /// Whether a path's value can be animated: `Ok(true)` it is keyed,
    /// `Ok(false)` it can be, `Err(why)` it cannot (a computed value);
    /// `None`: no key for it (a name of another value, words, a choice).
    pub animatable: Option<Box<dyn Fn(&str) -> Option<Result<bool, String>>>>,
    /// A path's value made keys (`true`: one key at its value now, its lane
    /// opened) or one constant again (`false`).
    pub animate: Option<Box<dyn Fn(&str, bool)>>,
    /// When in the film paths act: their spans in seconds.
    pub spans: Option<Box<dyn Fn(&[String]) -> Vec<(f64, f64)>>>,
    /// What the picture's selection is called, while there is one (the
    /// panel then shows only what it reads).
    pub selection_label: Option<Box<dyn Fn() -> Option<String>>>,
    /// The picture's selection cleared (the panel's "clear").
    pub clear_selection_filter: Option<Box<dyn Fn()>>,
    /// A scene the panel stands for (an edit's clip): its card.
    pub scene: Option<Box<dyn Fn(&str) -> Option<SceneCard>>>,
    /// A scene opened (its own panel).
    pub open: Option<Box<dyn Fn(&str)>>,
    /// A control's gesture begins (`true`: its sets are held live) and
    /// ends (`false`: what it held is written, one undo step), wherever
    /// the control is (a panel outside the editor panel's own area).
    pub hold: Option<Box<dyn Fn(bool)>>,
}

/// A scene of a film as a panel shows it: its name, its time in the film,
/// and its picture when the host has one.
#[derive(Clone)]
pub struct SceneCard {
    pub label: String,
    pub start: f64,
    pub end: f64,
    /// The film's length (the time bar's scale).
    pub film: f64,
    pub thumb: Option<makepad_widgets::Texture>,
}

impl PanelHost {
    /// A host that reads and sets paths; the rest is added as the host has
    /// it (`PanelHost { swap: Some(…), ..PanelHost::new(read, set) }`).
    pub fn new(read: impl Fn(&str) -> Option<PanelValue> + 'static, set: impl Fn(&str, PanelValue) + 'static) -> Self {
        Self { read: Box::new(read), set: Box::new(set), swap: None, fns_like: None, palette: None, recolour: None, frame_colours: None, saved_palettes: None, live: None, playhead: None, song: None, beats: None, words: None, ask: None, ask_status: None, focus: None, active: None, picked: None, lit_rev: None, reveal: None, animatable: None, animate: None, spans: None, selection_label: None, clear_selection_filter: None, scene: None, open: None, hold: None }
    }
}

thread_local! {
    static HOST: RefCell<Option<PanelHost>> = const { RefCell::new(None) };
    static REVISION: Cell<u64> = const { Cell::new(1) };
}

/// The host the panel's widgets read and write through (the desk mounting
/// the scene's panel).
pub fn install(host: PanelHost) {
    HOST.with(|h| *h.borrow_mut() = Some(host));
    forget_places();
    changed();
}

/// No host: widgets show their own values and only emit actions.
pub fn uninstall() {
    HOST.with(|h| *h.borrow_mut() = None);
    forget_places();
    changed();
}

/// The scene's code changed: bound widgets read their paths again.
pub fn changed() {
    REVISION.with(|r| r.set(r.get() + 1));
}

/// Which change of the scene the widgets last saw.
pub fn revision() -> u64 {
    REVISION.with(|r| r.get())
}

pub fn read(path: &str) -> Option<PanelValue> {
    if path.is_empty() {
        return None;
    }
    timed("host.read", || HOST.with(|h| h.borrow().as_ref().and_then(|h| (h.read)(path))))
}

/// Whether a host is installed (a bound widget writes through it).
pub fn hosted() -> bool {
    HOST.with(|h| h.borrow().is_some())
}

pub fn set(path: &str, value: PanelValue) {
    if path.is_empty() {
        return;
    }
    HOST.with(|h| {
        if let Some(h) = h.borrow().as_ref() {
            (h.set)(path, value)
        }
    });
}

/// A control's gesture begins or ends ([`PanelHost::hold`]).
pub fn hold(on: bool) {
    HOST.with(|h| {
        if let Some(f) = h.borrow().as_ref().and_then(|h| h.hold.as_ref()) {
            f(on)
        }
    });
}

pub fn swap(path: &str, name: &str) {
    HOST.with(|h| {
        if let Some(f) = h.borrow().as_ref().and_then(|h| h.swap.as_ref()) {
            f(path, name)
        }
    });
}

pub fn fns_like(path: &str) -> Vec<String> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.fns_like.as_ref().map(|f| f(path))).unwrap_or_default())
}

pub fn palette(scope: &str) -> Vec<Swatch> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.palette.as_ref().map(|f| f(scope))).unwrap_or_default())
}

/// Whether the host can recolour a palette.
pub fn can_recolour() -> bool {
    HOST.with(|h| h.borrow().as_ref().is_some_and(|h| h.recolour.is_some()))
}

pub fn recolour(scope: &str, map: Vec<([f64; 4], [f64; 4])>) {
    HOST.with(|h| {
        if let Some(f) = h.borrow().as_ref().and_then(|h| h.recolour.as_ref()) {
            f(scope, map)
        }
    });
}

pub fn frame_colours(n: usize) -> Vec<[f64; 4]> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.frame_colours.as_ref().map(|f| f(n))).unwrap_or_default())
}

pub fn live(path: &str) -> Option<f64> {
    timed("host.live", || HOST.with(|h| h.borrow().as_ref().and_then(|h| h.live.as_ref().and_then(|f| f(path)))))
}

/// The playhead (time, length, playing), when the host says.
/// A scene's card (an edit's clip), when the host has scenes.
pub fn scene(node: &str) -> Option<SceneCard> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.scene.as_ref().and_then(|f| f(node))))
}

/// Open a scene's own panel.
pub fn open(node: &str) {
    HOST.with(|h| {
        if let Some(f) = h.borrow().as_ref().and_then(|h| h.open.as_ref()) {
            f(node)
        }
    });
}

pub fn playhead() -> Option<(f64, f64, bool)> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.playhead.as_ref().map(|f| f())))
}

pub fn song(t0: f64, t1: f64, n: usize) -> Vec<f32> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.song.as_ref().map(|f| f(t0, t1, n))).unwrap_or_default())
}

pub fn beats(t0: f64, t1: f64) -> Vec<f64> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.beats.as_ref().map(|f| f(t0, t1))).unwrap_or_default())
}

pub fn words(t0: f64, t1: f64) -> Vec<(f64, f64, String)> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.words.as_ref().map(|f| f(t0, t1))).unwrap_or_default())
}

/// Whether the host takes asks (the AI button shows only then).
pub fn can_ask() -> bool {
    HOST.with(|h| h.borrow().as_ref().is_some_and(|h| h.ask.is_some()))
}

pub fn ask(prompt: &str, targets: &[String], mode: &str) -> Option<u64> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.ask.as_ref().and_then(|f| f(prompt, targets, mode))))
}

pub fn ask_status(id: u64) -> String {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.ask_status.as_ref().map(|f| f(id))).unwrap_or_else(|| "failed: no host".into()))
}

pub fn saved_palettes() -> Vec<(String, Vec<[f64; 4]>)> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.saved_palettes.as_ref().map(|f| f())).unwrap_or_default())
}

/// A path's last part, said as a label (`WORD_SPARKS.count` → "Count",
/// `camera_keys[3].zoom` → "Zoom").
pub fn label_of(path: &str) -> String {
    let last = path.rsplit(['.', '[']).next().unwrap_or(path).trim_end_matches(']');
    let words = last.replace('_', " ").to_lowercase();
    let mut c = words.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Whether a script callback was given (`on_change: |v| {…}`).
pub fn bound(f: &makepad_widgets::makepad_script::ScriptFnRef) -> bool {
    f.as_object() != makepad_widgets::makepad_script::ScriptObject::ZERO
}

// ---------------------------------------------------------------- modules

/// What a bound control is, as a module sees it (its presets, its morph,
/// its randomise).
#[derive(Clone, Debug, PartialEq)]
pub enum BoundKind {
    /// Numbers, moved within `lo..hi` (each number of the value).
    Numbers { lo: f64, hi: f64, step: f64 },
    Colour,
    Bool,
    /// One of these (written as the value was: `@name`, `"words"`).
    Choice(Vec<String>),
    /// Anything else (a font, words): stored and recalled, not morphed.
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bound {
    pub path: String,
    pub kind: BoundKind,
}

/// A module's controls, as they drew this frame, and its stored values.
#[derive(Clone, Debug, Default)]
pub struct ModuleState {
    pub bounds: Vec<Bound>,
    /// Slot values: each path's value as code.
    pub slots: Vec<Option<Vec<(String, String)>>>,
    /// The values the module found when it first drew (reset).
    pub original: Vec<(String, String)>,
}

thread_local! {
    /// The modules being drawn (innermost last): what registers into.
    static DRAWING: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static MODULES: RefCell<std::collections::HashMap<u64, ModuleState>> = RefCell::new(std::collections::HashMap::new());
}

/// A module starts drawing its controls (`id`: the module widget's uid).
pub fn module_begin(id: u64) {
    DRAWING.with(|d| d.borrow_mut().push(id));
    DRAW_SERIAL.with(|n| {
        n.set(n.get() + 1);
        MODULE_DRAWN.with(|m| m.borrow_mut().insert(id, n.get()));
    });
    MODULES.with(|m| m.borrow_mut().entry(id).or_default().bounds.clear());
}

/// A module has drawn its controls.
pub fn module_end() {
    DRAWING.with(|d| d.borrow_mut().pop());
}

/// The module being drawn, if any.
pub fn module_current() -> Option<u64> {
    DRAWING.with(|d| d.borrow().last().copied())
}

/// A bound control says what it is to the module drawing it (every module
/// it is in, so an outer module's presets cover inner ones).
pub fn register(bound: Bound) {
    if bound.path.is_empty() {
        return;
    }
    let ids: Vec<u64> = DRAWING.with(|d| d.borrow().clone());
    MODULES.with(|m| {
        let mut m = m.borrow_mut();
        for id in ids {
            let state = m.entry(id).or_default();
            if !state.bounds.iter().any(|b| b.path == bound.path) {
                if !state.original.iter().any(|(p, _)| *p == bound.path) {
                    if let Some(v) = read(&bound.path) {
                        state.original.push((bound.path.clone(), v.code()));
                    }
                }
                state.bounds.push(bound.clone());
            }
        }
    });
}

/// A module's state (a copy).
pub fn module(id: u64) -> ModuleState {
    MODULES.with(|m| m.borrow().get(&id).cloned().unwrap_or_default())
}

/// A module's state changed.
pub fn module_update(id: u64, f: impl FnOnce(&mut ModuleState)) {
    MODULES.with(|m| f(m.borrow_mut().entry(id).or_default()));
}

/// Every bound path's value now, as code.
pub fn snapshot(bounds: &[Bound]) -> Vec<(String, String)> {
    bounds.iter().filter_map(|b| Some((b.path.clone(), read(&b.path)?.code()))).collect()
}

// ---------------------------------------------------------------- animation

/// Whether a path can be animated, when the host says (`None`: no key
/// button).
pub fn animatable(path: &str) -> Option<Result<bool, String>> {
    if path.is_empty() {
        return None;
    }
    timed("host.animatable", || HOST.with(|h| h.borrow().as_ref().and_then(|h| h.animatable.as_ref().and_then(|f| f(path)))))
}

/// A path's animation taken away whole (its value at the playhead stays).
pub fn animate_all(path: &str, on: bool) {
    // Said to the host as an animate with a marker: `!path` removes it all.
    animate(&format!("!{path}"), on);
}

pub fn animate(path: &str, on: bool) {
    HOST.with(|h| {
        if let Some(f) = h.borrow().as_ref().and_then(|h| h.animate.as_ref()) {
            f(path, on)
        }
    });
    changed();
}

thread_local! {
    /// A note for the pointer (a key button's, or its offer), drawn by the
    /// module over its controls: where, what, and whether it is the offer.
    static TIP: RefCell<Option<(Rect, String, bool)>> = const { RefCell::new(None) };
}

/// A control asks for a note at `rect`; true when a module will draw it.
pub fn tip(rect: Rect, note: String, offer: bool) -> bool {
    if module_current().is_none() {
        return false;
    }
    TIP.with(|t| *t.borrow_mut() = Some((rect, note, offer)));
    true
}

pub fn take_tip() -> Option<(Rect, String, bool)> {
    TIP.with(|t| t.borrow_mut().take())
}

/// A control's key button: where it is, what the host said, whether the
/// pointer is on it and whether "Remove animation" is offered.
#[derive(Clone, Debug, Default)]
pub struct KeyButton {
    pub state: Option<Result<bool, String>>,
    seen: u64,
    pub rect: Rect,
    pub hot: bool,
    pub offering: bool,
    pub remove: Rect,
}

impl KeyButton {
    /// What the host says of `path`, asked again when the scene changed.
    pub fn refresh(&mut self, path: &str) {
        let rev = revision();
        if self.seen != rev {
            self.seen = rev;
            self.state = animatable(path);
            if self.state != Some(Ok(true)) {
                self.offering = false;
            }
        }
    }

    pub fn keyed(&self) -> bool {
        self.state == Some(Ok(true))
    }

    /// Whether `abs` is on the button or on its "Remove animation".
    pub fn contains(&self, abs: DVec2) -> bool {
        self.state.is_some() && (self.rect.contains(abs) || (self.offering && self.remove.contains(abs)))
    }

    /// A click at `abs` (true when the button took it): keys a value that
    /// can be keyed, offers "Remove animation" on a keyed one, and takes
    /// that.
    pub fn click(&mut self, path: &str, abs: DVec2) -> bool {
        if self.offering && self.remove.contains(abs) {
            self.offering = false;
            // The offer is "Remove animation" on a keyed value, "Animate" on one that can be.
            match self.state {
                Some(Ok(true)) => animate_all(path, false),
                Some(Ok(false)) => animate(path, true),
                _ => {}
            }
            return true;
        }
        if !self.rect.contains(abs) {
            return false;
        }
        match &self.state {
            Some(Ok(false)) => animate(path, true),
            // An animated value's cue: its curve shown.
            Some(Ok(true)) => animate(&format!("?{path}"), true),
            _ => {}
        }
        true
    }

    /// A secondary click on the control: "Remove animation" offered when
    /// it is keyed.
    pub fn context(&mut self) -> bool {
        if matches!(self.state, Some(Ok(_))) {
            self.offering = true;
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------- time and space

/// Where a bound control (or a module) drew this frame: its area (found
/// again however the panel scrolls), its rect as drawn, its paths, and the
/// module it sits in.
struct Place {
    uid: u64,
    area: Area,
    rect: Rect,
    paths: Vec<String>,
    module: bool,
    owner: Option<u64>,
    /// Which drawing of its module placed it (a control folded away since
    /// keeps its last place, which is not where anything is now).
    drawn: u64,
}

thread_local! {
    static DRAW_SERIAL: Cell<u64> = const { Cell::new(0) };
    /// Each module's latest drawing.
    static MODULE_DRAWN: RefCell<std::collections::HashMap<u64, u64>> = RefCell::new(std::collections::HashMap::new());
}

thread_local! {
    static PLACES: RefCell<Vec<Place>> = const { RefCell::new(Vec::new()) };
    /// What the pointer is over, the selected module, and the focus last
    /// said to the host.
    static HOVER: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    static SELECTED: RefCell<Option<(u64, Vec<String>)>> = const { RefCell::new(None) };
    static SAID: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    static ONLY_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static REVEALED: Cell<u64> = const { Cell::new(0) };
}

/// The panel's program was mounted again (made, remade, fixed): what the
/// old one placed (its modules in the header's index, its controls) is
/// forgotten.
pub fn program_changed() {
    forget_places();
}

/// A host installed (or none): the panel's widgets and what they recorded
/// stay (hosts take turns, a frame each, where several pages hold a
/// panel); only the focus is said again to the host now in.
fn forget_places() {
    SAID.with(|s| *s.borrow_mut() = None);
}

/// A bound control drew at `rect` (its paths): kept so the pointer over it
/// says its paths and its module can dim it.
pub fn placed(cx: &mut Cx2d, area: &mut Area, uid: u64, rect: Rect, paths: Vec<String>) {
    place(cx, area, uid, rect, paths, false);
}

/// A module drew at `rect` (every path in it).
pub fn placed_module(cx: &mut Cx2d, area: &mut Area, uid: u64, rect: Rect, paths: Vec<String>) {
    place(cx, area, uid, rect, paths, true);
}

fn place(cx: &mut Cx2d, area: &mut Area, uid: u64, rect: Rect, paths: Vec<String>, module: bool) {
    cx.add_aligned_rect_area(area, rect);
    let owner = DRAWING.with(|d| d.borrow().iter().rev().find(|m| **m != uid).copied());
    let drawn = owner.and_then(|o| MODULE_DRAWN.with(|m| m.borrow().get(&o).copied())).unwrap_or(0);
    let place = Place { uid, area: *area, rect, paths, module, owner, drawn };
    PLACES.with(|p| {
        let mut p = p.borrow_mut();
        match p.iter_mut().find(|q| q.uid == uid) {
            Some(q) => *q = place,
            None => p.push(place),
        }
    });
}

/// The controls module `id` holds directly: each rect as drawn and paths.
pub fn module_places(id: u64) -> Vec<(Rect, Vec<String>)> {
    let now = MODULE_DRAWN.with(|m| m.borrow().get(&id).copied());
    PLACES.with(|p| p.borrow().iter().filter(|q| !q.module && q.owner == Some(id) && Some(q.drawn) == now).map(|q| (q.rect, q.paths.clone())).collect())
}

/// The pointer moved to `abs`: the smallest control or module under it is
/// what the person is on.
pub fn pointer(cx: &Cx, abs: DVec2) {
    let over = PLACES.with(|p| {
        p.borrow()
            .iter()
            .map(|q| (q.area.clipped_rect(cx), q))
            .filter(|(r, q)| r.contains(abs) && !q.paths.is_empty())
            .min_by(|a, b| (a.0.size.x * a.0.size.y).total_cmp(&(b.0.size.x * b.0.size.y)))
            .map(|(_, q)| q.paths.clone())
    });
    HOVER.with(|h| *h.borrow_mut() = over);
    say_focus();
}

/// A click on a module's header: it is selected (its paths stay the focus
/// until another click on it, or Escape); true when it is now selected.
pub fn select_module(id: u64, paths: Vec<String>) -> bool {
    let on = SELECTED.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().is_some_and(|(m, _)| *m == id) {
            *s = None;
            false
        } else {
            *s = Some((id, paths));
            true
        }
    });
    say_focus();
    on
}

/// Escape: no module selected.
pub fn clear_selection() {
    if SELECTED.with(|s| s.borrow_mut().take()).is_some() {
        say_focus();
    }
}

/// The selected module, if any.
pub fn selected_module() -> Option<u64> {
    SELECTED.with(|s| s.borrow().as_ref().map(|(m, _)| *m))
}

/// The host is told the focus when it changes (the pointer's, else the
/// selected module's).
fn say_focus() {
    let now = HOVER.with(|h| h.borrow().clone()).or_else(|| SELECTED.with(|s| s.borrow().as_ref().map(|(_, p)| p.clone())));
    if SAID.with(|s| *s.borrow() == now) {
        return;
    }
    SAID.with(|s| *s.borrow_mut() = now.clone());
    HOST.with(|h| {
        if let Some(f) = h.borrow().as_ref().and_then(|h| h.focus.as_ref()) {
            f(now.as_deref())
        }
    });
}

pub fn active(path: &str) -> Option<bool> {
    timed("host.active", || HOST.with(|h| h.borrow().as_ref().and_then(|h| h.active.as_ref().and_then(|f| f(path)))))
}

/// Whether the picture's selection reads one of `paths`.
pub fn picked(paths: &[String]) -> bool {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.picked.as_ref().map(|f| paths.iter().any(|p| f(p)))).unwrap_or(false))
}

/// Whether a set of paths matters now: active when one is, inactive only
/// when every one is known not to be (a path not known may be in use: in
/// doubt, not dimmed).
pub fn activity(paths: &[String]) -> Option<bool> {
    let mut all_known = !paths.is_empty();
    for p in paths {
        match active(p) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => all_known = false,
        }
    }
    all_known.then_some(false)
}

pub fn lit_rev() -> u64 {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.lit_rev.as_ref().map(|f| f())).unwrap_or(0))
}

/// Whether the picture's selection changed since the panel last showed
/// it: true once per change.
pub fn take_reveal() -> bool {
    let now = HOST.with(|h| h.borrow().as_ref().and_then(|h| h.reveal.as_ref().map(|f| f())).unwrap_or(0));
    REVEALED.with(|r| r.replace(now)) != now
}

/// The first active module's rect as drawn (where the panel scrolls to
/// when the picture's selection changes).
pub fn first_active_rect() -> Option<Rect> {
    let modules: Vec<(Rect, Vec<String>)> = PLACES.with(|p| p.borrow().iter().filter(|q| q.module).map(|q| (q.rect, q.paths.clone())).collect());
    modules.into_iter().filter(|(_, paths)| activity(paths) == Some(true)).map(|(r, _)| r).min_by(|a, b| a.pos.y.total_cmp(&b.pos.y))
}

/// "Only what's active": modules with no active path are not drawn.
pub fn only_active() -> bool {
    ONLY_ACTIVE.with(|o| o.get())
}

pub fn set_only_active(on: bool) {
    ONLY_ACTIVE.with(|o| o.set(on));
    changed();
}

// ---------------------------------------------------------------- the tree

pub fn spans(paths: &[String]) -> Vec<(f64, f64)> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.spans.as_ref().map(|f| f(paths))).unwrap_or_default())
}

pub fn selection_label() -> Option<String> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.selection_label.as_ref().and_then(|f| f())))
}

pub fn clear_selection_filter() {
    HOST.with(|h| {
        if let Some(f) = h.borrow().as_ref().and_then(|h| h.clear_selection_filter.as_ref()) {
            f()
        }
    });
    changed();
}

/// The picture's selection's count (not taken: each module and group
/// keeps the one it last acted on).
pub fn reveal_rev() -> u64 {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.reveal.as_ref().map(|f| f())).unwrap_or(0))
}

thread_local! {
    /// The panel's modules (top to bottom): id, title, where it last drew,
    /// and whether it is shown (not filtered out). An entry lives as long
    /// as its module (a module forgets itself when dropped), not by what a
    /// redraw drew: a pass that redraws part of the panel changes nothing.
    static TREE: RefCell<Vec<(u64, String, Rect, bool)>> = const { RefCell::new(Vec::new()) };
    static SEARCH: RefCell<String> = const { RefCell::new(String::new()) };
    static TITLE_FOUND: Cell<bool> = const { Cell::new(false) };
    static OPEN: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static SCROLL_TO: RefCell<Option<Rect>> = const { RefCell::new(None) };
}

/// A module drew (its title and where).
pub fn tree_module(id: u64, title: &str, rect: Rect) {
    TREE.with(|t| {
        let mut t = t.borrow_mut();
        match t.iter_mut().find(|m| m.0 == id) {
            Some(m) => *m = (id, title.to_string(), rect, true),
            None => t.push((id, title.to_string(), rect, true)),
        }
        t.sort_by(|a, b| a.2.pos.y.total_cmp(&b.2.pos.y));
    });
}

/// A module is filtered out (only-active, the selection, the search): not
/// in the index until it is shown again.
pub fn tree_hidden(id: u64, title: &str) {
    TREE.with(|t| {
        let mut t = t.borrow_mut();
        match t.iter_mut().find(|m| m.0 == id) {
            Some(m) => m.3 = false,
            None => t.push((id, title.to_string(), Rect::default(), false)),
        }
    });
}

/// A module is gone (its panel made again or closed).
pub fn tree_forget(id: u64) {
    TREE.with(|t| t.borrow_mut().retain(|m| m.0 != id));
    MODULE_DRAWN.with(|m| m.borrow_mut().remove(&id));
    MODULES.with(|m| m.borrow_mut().remove(&id));
}

/// The elements, top to bottom: each title once, with the modules of
/// that title (an element's overview, timing, shape and details).
pub fn tree_elements() -> Vec<(String, Vec<u64>)> {
    TREE.with(|t| {
        let mut out: Vec<(String, Vec<u64>)> = Vec::new();
        for m in t.borrow().iter().filter(|m| m.3) {
            match out.iter_mut().find(|e| e.0 == m.1) {
                Some(e) => e.1.push(m.0),
                None => out.push((m.1.clone(), vec![m.0])),
            }
        }
        out
    })
}

/// The search over controls (their paths and labels); empty: all shown.
pub fn search() -> String {
    SEARCH.with(|s| s.borrow().clone())
}

pub fn set_search(text: &str) {
    SEARCH.with(|s| *s.borrow_mut() = text.trim().to_lowercase());
    changed();
}

/// Whether a control with these paths is shown under the search.
pub fn found(paths: &[String]) -> bool {
    let q = search();
    q.is_empty() || TITLE_FOUND.with(|t| t.get()) || paths.iter().any(|p| p.to_lowercase().contains(&q) || label_of(p).to_lowercase().contains(&q))
}

/// Whether the search finds `title` (a module's): the module is shown
/// whole.
pub fn title_found(title: &str) -> bool {
    let q = search();
    !q.is_empty() && title.to_lowercase().contains(&q)
}

/// While a module whose title the search finds draws its controls, all of
/// them are found.
pub fn set_title_found(on: bool) {
    TITLE_FOUND.with(|t| t.set(on));
}

/// The modules whose titles the search finds (title, ids), for the header
/// to jump to.
pub fn tree_titles_found() -> Vec<(String, Vec<u64>)> {
    let q = search();
    if q.is_empty() {
        return Vec::new();
    }
    TREE.with(|t| {
        let mut out: Vec<(String, Vec<u64>)> = Vec::new();
        for m in t.borrow().iter().filter(|m| m.1.to_lowercase().contains(&q)) {
            match out.iter_mut().find(|e| e.0 == m.1) {
                Some(e) => e.1.push(m.0),
                None => out.push((m.1.clone(), vec![m.0])),
            }
        }
        out
    })
}

/// Modules `ids` (an element's) are asked to open, and the first to be
/// scrolled to.
pub fn open_modules(ids: &[u64]) {
    OPEN.with(|o| o.borrow_mut().extend_from_slice(ids));
    if let Some(rect) = TREE.with(|t| t.borrow().iter().filter(|m| ids.contains(&m.0)).map(|m| m.2).min_by(|a, b| a.pos.y.total_cmp(&b.pos.y))) {
        SCROLL_TO.with(|s| *s.borrow_mut() = Some(rect));
    }
}

/// Whether module `id` was asked to open (once).
pub fn take_open(id: u64) -> bool {
    OPEN.with(|o| {
        let mut o = o.borrow_mut();
        let had = o.contains(&id);
        o.retain(|m| *m != id);
        had
    })
}

/// Where the host's scroll should go (a module asked for from the index),
/// once: its rect as drawn.
pub fn take_scroll_to() -> Option<Rect> {
    SCROLL_TO.with(|s| s.borrow_mut().take())
}

// ---------------------------------------------------------------- step timing

thread_local! {
    /// Time spent per named part since the last [`times_take`] (trace
    /// `panel.step`): what a drag step costs, part by part.
    static TIMES: RefCell<Vec<(&'static str, f64, u32)>> = const { RefCell::new(Vec::new()) };
}

/// Whether step timing is on (`MAKEPAD_TRACE=panel.step`).
pub fn timing() -> bool {
    makepad_widgets::makepad_platform::makepad_error_log::trace_enabled("panel.step")
}

/// `secs` more spent in `part`.
pub fn time_add(part: &'static str, secs: f64) {
    TIMES.with(|t| {
        let mut t = t.borrow_mut();
        match t.iter_mut().find(|(p, ..)| *p == part) {
            Some(e) => {
                e.1 += secs;
                e.2 += 1;
            }
            None => t.push((part, secs, 1)),
        }
    });
}

/// Run `f`, its time added to `part` when timing is on.
pub fn timed<R>(part: &'static str, f: impl FnOnce() -> R) -> R {
    if !timing() {
        return f();
    }
    let t0 = makepad_widgets::Cx::time_now();
    let r = f();
    time_add(part, makepad_widgets::Cx::time_now() - t0);
    r
}

/// The parts timed since the last call, as `part ms(×calls)` sorted by time.
pub fn times_take() -> String {
    let mut t = TIMES.with(|t| std::mem::take(&mut *t.borrow_mut()));
    t.sort_by(|a, b| b.1.total_cmp(&a.1));
    t.iter().filter(|(_, s, _)| *s >= 0.0005).map(|(p, s, n)| format!("{p} {:.1}ms(x{n})", s * 1000.0)).collect::<Vec<_>>().join(" · ")
}

/// Bring `rect` (as drawn) into view, as a module asked for is.
pub fn scroll_to(rect: Rect) {
    SCROLL_TO.with(|s| *s.borrow_mut() = Some(rect));
}

// ---------------------------------------------------------------- folding

thread_local! {
    /// What the person folded or opened by hand, by what it is (a module's
    /// or a group's title and first path): a panel made again (remade,
    /// rebuilt, its program mounted anew) keeps it, and only a NEW
    /// selection in the picture opens it again.
    static FOLDS: RefCell<std::collections::HashMap<String, bool>> = RefCell::new(std::collections::HashMap::new());
    /// Something just opened by hand (a module, a group, a picker), where
    /// it drew: the panel brings it into view, its heading staying put.
    static OPENED: Cell<Option<Rect>> = const { Cell::new(None) };
}

/// A fold's key: its title and the first path it holds.
pub fn fold_key(title: &str, paths: &[String]) -> String {
    format!("{title}|{}", paths.first().map_or("", |p| p.as_str()))
}

/// Open (`true`) or folded as the person last left it, if they did.
pub fn folded_by_hand(key: &str) -> Option<bool> {
    FOLDS.with(|f| f.borrow().get(key).copied())
}

/// The person opened (`true`) or folded it.
pub fn set_fold(key: &str, open: bool) {
    FOLDS.with(|f| f.borrow_mut().insert(key.to_string(), open));
}

/// What just opened by hand drew at `rect` (as drawn, before the scroll).
pub fn opened(rect: Rect) {
    OPENED.with(|o| o.set(Some(rect)));
}

/// What just opened, once: the panel scrolls it into view.
pub fn take_opened() -> Option<Rect> {
    OPENED.with(|o| o.take())
}
