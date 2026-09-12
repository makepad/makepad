//! The TWEAKER — the design-feedback overlay every `--remote` app grows.
//!
//! Hardcoded into `Window` (like the caption bar: zero app wiring), inert
//! unless the remote bridge is live, zero cost while off. Turned on (Shift+F10 or
//! `GET /tweak?on=1`), a person points at the UI and live-edits it while the
//! AI watches the same session through the bridge:
//!
//! * pointer events over the window body are swallowed BEFORE ordinary
//!   widget dispatch (a Button under the cursor outlines, it never fires)
//!   and resolved against the live widget tree into a pick: id path, type,
//!   rect, margin/padding band;
//! * the selected widget's `#[source] ScriptObjectRef` is reflected (own
//!   map = explicitly set values, proto chain = the type's live/shader-input
//!   registry) into a real property list — no synthetic schema;
//! * splash chunks are live-loaded onto the one selected instance through
//!   the ordinary apply machinery (`script_apply_eval` semantics, `+:`
//!   merge rules intact) followed by a full redraw/relayout pass, and every
//!   applied chunk lands in the session diff log as (path, prop, old, new);
//! * `/tweak/final` answers the coalesced end state so the AI integrates
//!   once instead of tracking every intermediate edit.
//!
//! # The Spec tab and the prompt strip — the human's side of the channel
//!
//! Everything above is the AI reading the app. The SPEC tab is the person
//! writing back. It carries three fields: NOTES, what a widget is about,
//! written for whoever reads it next; RULES, what must stay true of it; and
//! under a divider the rules that stand over the whole APP. A note describes
//! and a rule constrains, and they are kept apart so that an agent reading
//! the state can tell "here is context" from "do not break this". The
//! app-wide field is why the tab works with nothing selected at all.
//!
//! `Insert` or `Ctrl+Shift+N` shows the tab on the selection (the panel's
//! `note` button does the same for keyboards without an Insert), and so does
//! a click on a pin badge or a right click on a widget. A badge means
//! something is written about that widget.
//!
//! The PROMPT is one box pinned below the tabs, outside them, so whichever
//! tab is up the box is in the same place and the tab body scrolls above it.
//! One box for the whole panel, not one per widget: a send attributes what
//! is in it to whatever is selected at that moment, and with nothing
//! selected the ask is about the app. `Ctrl+Enter` sends, `Alt+Enter`
//! queues a message to go out with the next send, and Up and Down walk what
//! was sent before while the box is empty.
//!
//! Sending does not push anything — the bridge is a server — it raises the
//! record's `sent` count, which `/tweak/state` reports as `"ask": N` and the
//! log ring carries as `TWEAK ask #N`. A polling agent reads either and
//! knows the difference between a note left lying around and one it is being
//! asked to act on NOW.
//!
//! Notes and rules are written to `.makepad-notes.txt` beside the running
//! app and the app-wide document to `.makepad-rules.txt` — plain
//! tab-separated text — so both survive the process and are readable
//! without it.
//!
//! Typing `@` into a note arms a widget pick: the hover turns amber and the
//! next click writes that widget's reference into the note, leaving the
//! selection alone. A pinned note also marks its widget with a small amber
//! pin, which opens it.
//!
//! References are readable paths ([`readable_paths`]): the widget's name
//! where it has one, its type where it does not, `_1`/`_2` only where
//! siblings collide, and the head every path in the app shares dropped, so
//! they start where the app does — `dock.tOverview.View.View.Label_1`. That
//! is what the footer copies and what a note is keyed by. An `@` writes the
//! same thing, or the shorter relative form when there is one: `./Label_2`
//! for a sibling, `../Button` one container out.
//!
//! And with a selection standing, the arrow keys walk the live tree the way
//! a scene editor does: parent, first child, previous/next sibling. With
//! nothing selected they still belong to the exploded view's orbit.
//!
//! Two smaller readouts round the selection out: a full-width line at the
//! top of the Layout section prints what the layout actually produced, above
//! the Fill/Fit/number controls that asked for it, and clicking the footer's
//! path line copies the whole path
//! to the clipboard — the panel can only show it head-clipped, and a path
//! you cannot select is a path you cannot quote.
//!
//! Containment (the plan of record, tweaker.md): everything UI-side lives
//! HERE; `Window` hosts the widget and calls [`window_intercept`] — a few
//! lines; the `/tweak` routes in `platform/src/remote.rs` stay thin and
//! delegate through `Cx::tweak_callback`, registered by `set_ui_root`
//! exactly like the widget-tree callbacks, so platform never depends on
//! widgets. The overlay reads and applies — only the AI writes source.

use crate::{
    check_box::{CheckBox, CheckBoxAction},
    fab_controls::{format_hex, parse_hex, rgb_to_hsv, FabColorPick, FabColorPickAction, FabValueInput, FabValueInputAction, FabValueInputWidgetRefExt},
    makepad_draw::makepad_platform::sploded::{SPLODED_SPREAD_DEFAULT, SPLODED_SPREAD_MAX, SPLODED_SPREAD_MIN},
    dock::DockWidgetRefExt,
    file_tree::{FileTree, FileTreeAction},
    fold_header::FoldHeaderWidgetRefExt,
    page_flip::PageFlipWidgetRefExt,
    label::Label,
    makepad_derive_widget::*,
    makepad_draw::*,
    portal_list::PortalList,
    text_input::TextInputAction,
    view::View,
    widget::*,
    widget_tree::{live_id_token, widget_type_names, CxWidgetExt},
};
use crate::makepad_script::script_eval;
use crate::Animate;
use crate::ButtonAction;
use crate::tooltip::Tooltip;
use crate::animator::{AnimatorState, Ease as AnimEase, Play};
use crate::makepad_draw::makepad_platform::DrawShaderId;
use crate::makepad_script::trap::NoTrap;
use crate::makepad_script::{parse_doc_hint, ScriptHeap, ScriptMod, ScriptObject};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// session state — one tweak session per app, shared by every window's
// Tweaker instance and by the remote callback (all main-thread; the Mutex is
// for the static, not for concurrency).
// ---------------------------------------------------------------------------

/// Fast zero-cost-when-off gate: one relaxed atomic load per event.
static TWEAK_ON: AtomicBool = AtomicBool::new(false);

pub fn tweak_is_on() -> bool {
    TWEAK_ON.load(Ordering::Relaxed)
}

/// One resolved widget under the pointer (or pinned by a click).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TweakPick {
    pub uid: u64,
    /// Dotted id path from the ui root, the same tokens `/d` prints.
    pub path: String,
    pub ty: String,
    /// Window-local rect.
    pub rect: Rect,
    pub window_id: usize,
    /// `Some("padding")` / `Some("margin:<child>")` when the pointer sits in
    /// the spacing band rather than on content — the gap between rects is a
    /// first-class target.
    pub band: Option<String>,
    /// The plane the widget renders on in the exploded view (its component
    /// nesting depth); 0 when it has not drawn while the mode was up.
    pub level: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum PickStyle {
    Hover,
    /// Hovering while a note's `@` is waiting for a widget: amber, and
    /// heavier than the ordinary hover, because this click does something
    /// different — it writes a name into the note instead of selecting.
    Mention,
    Pinned,
    /// Pinned while a value is actively moving: hairline stipple only.
    PinnedQuiet,
}

#[derive(Clone, Debug)]
pub struct TweakDiffEntry {
    pub seq: u64,
    pub path: String,
    pub prop: String,
    pub old: String,
    pub new: String,
    /// Where the widget's source object was constructed — `file:line` of
    /// the literal to edit. For a widget built from a template (a tab, a
    /// list item) this is the TEMPLATE's `:=` site, not the instance: that
    /// is what the AI rewrites so every instance follows.
    pub origin: String,
    /// How many other live widgets share that source object and received
    /// the same edit (0 for an ordinary, one-off widget).
    pub siblings: u32,
    /// "this" — specialise this instance (origin = its own site) — or
    /// "all" — every widget of the type (origin = the type's definition).
    pub scope: String,
}

/// A note card, attached to a widget by path: it rides with the widget's
/// live rect at (dx, dy) offset, sized (w, h), and is the human's text
/// channel to the AI (/tweak/state carries it).
#[derive(Clone, Debug)]
pub struct TweakNote {
    pub path: String,
    /// The NOTE: what this widget is about, written for whoever reads it
    /// next — you, tomorrow, or an agent looking for standing context. It is
    /// never cleared by sending; only you empty it.
    pub text: String,
    /// The RULES: what must stay true of this widget. A note describes, a
    /// rule constrains -- they are kept apart because an agent reading the
    /// state must be able to tell "here is context" from "do not break
    /// this". Saved beside the note.
    pub rules: String,
    /// Bumped every time the human sends the note to the AI (the sparkle
    /// button / Ctrl+Enter). `/tweak/state` reports the note as `ask` while
    /// this is above the count the AI last acknowledged, so a polling agent
    /// can tell "there is a note here" from "act on this note NOW".
    pub sent: u64,
}


/// How much one wheel notch changes the magnification.
const ZOOM_WHEEL_STEP: f32 = 0.15;

/// How much one wheel notch opens or closes the exploded stack.
const SPREAD_WHEEL_STEP: f32 = 0.04;

/// What a badge says about its widget. A mark this size has one thing to
/// say with -- its colour -- so it says the most useful thing: whether what
/// is written here is a description, a constraint, or both.
#[derive(Clone, Copy, PartialEq)]
enum BadgeKind {
    /// A note only: someone described this.
    Note,
    /// A rule only: something here must stay true.
    Rule,
    /// Both.
    Both,
}

impl BadgeKind {
    /// Which of the three pre-coloured pins draws this badge. They are three
    /// separate widgets rather than one recoloured per badge: an apply does
    /// not land before the draw_walk that follows it, so a single shared
    /// icon paints each badge in the NEXT badge's colour.
    fn index(self) -> usize {
        match self {
            BadgeKind::Note => 0,
            BadgeKind::Rule => 1,
            BadgeKind::Both => 2,
        }
    }
}

/// The least a Spec field may be squeezed to: two lines plus the padding.
/// Below that a box says less than its own placeholder. It is also, in the
/// other direction, the only ceiling a field has -- one can grow until the
/// others are at their floor, and no further, because the three share the
/// tab's height rather than adding to it.
const SPEC_FIELD_MIN: f64 = 48.0;

/// How the Spec tab's height is shared between its three fields. These are
/// flex weights -- `grid-template-rows: repeat(3, minmax(48px, 1fr))`, in
/// effect -- so the split is a proportion of whatever height the tab has,
/// and a window that changes size keeps the proportion rather than an
/// absolute number that no longer fits. Zero means "never dragged" and
/// resolves to an equal share. Like the panel's own width they live on the
/// session and not on disk, so a drag lasts the run. Notes, rules, app
/// rules, in that order.
fn spec_weight(index: usize) -> f64 {
    let weights = session().lock().unwrap().spec_weights;
    // "Never dragged" is all three at zero, not this one: a drag writes the
    // whole triple, and a row dragged down to its floor legitimately holds
    // a weight of exactly zero -- treating that as unset handed it an equal
    // share back the moment it got there.
    if weights.iter().all(|w| *w <= 0.0) {
        1.0
    } else {
        weights[index].max(0.0)
    }
}

/// One message waiting in the strip's queue. Taken on the tab it was
/// written on: from the Shader tab it carries the draw layer that tab was
/// showing and the fn sources for it, and goes out as a shader-fn rewrite
/// rather than a plain ask -- the same channel the Shader tab's own box
/// used to be, folded into the one box everybody types into.
#[derive(Clone, Debug)]
pub struct Outgoing {
    pub path: String,
    pub text: String,
    /// (layer, fn sources) when this is about a shader.
    pub shader: Option<(String, String)>,
}

/// The pin badge's clickable square, in points.
const BADGE_SIZE: f64 = 13.0;
/// How often the badge targets are re-resolved (a whole-tree walk).
const BADGE_REFRESH: f64 = 0.5;

/// How far the pinned widget's dashed ring stands off the widget itself.
/// The leader measures to THAT — the outline is what the eye sees as the
/// edge of the selection.
const SELECTION_RING_OUTSET: f64 = 4.0;

impl TweakNote {
    fn new(path: String) -> Self {
        Self {
            path,
            text: String::new(),
            rules: String::new(),
            sent: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// the note store — pinned notes outlive the process
//
// One line per note, tab-separated, in the process's working directory. A
// flat text file on purpose: `cat .makepad-notes.txt` is a readable list of
// what the human asked for, and the AI that drives the session can read it
// without the app running.

const NOTE_STORE: &str = ".makepad-notes.txt";
/// The rules that stand over the WHOLE app, in their own file beside the
/// notes. Its own file rather than a row in the note store because it is
/// not about a widget: nothing keys it, and a person editing it by hand
/// should not have to find it among a hundred paths.
const RULES_STORE: &str = ".makepad-rules.txt";

/// Read the app-wide rules back. A missing file is an empty document, not
/// an error -- most apps will never have one.
fn app_rules_load() -> String {
    std::fs::read_to_string(RULES_STORE).unwrap_or_default()
}

/// Write the app-wide rules out, or take the file away when they are
/// emptied -- an empty file beside the app says something is there when
/// nothing is.
fn app_rules_save(text: &str) {
    if text.trim().is_empty() {
        let _ = std::fs::remove_file(RULES_STORE);
        return;
    }
    if let Err(error) = std::fs::write(RULES_STORE, text) {
        log!("TWEAK rules store write failed: {error}");
    }
}
/// Custom names live beside the notes and outlive the process the same way:
/// a name typed into the Props tab is a request the AI carries out in the
/// source, and it must still be there when the AI gets to it — including
/// after a rebuild, which is exactly when a rename lands.
const NAME_STORE: &str = ".makepad-names.txt";
/// Layout conversions asked for on the Props tab -- a View that should be
/// a Grid, or the other way -- one per line, the same three columns as the
/// name store. A widget cannot change its type while it runs; the agent
/// edits the source, and the ask survives here until it has.
const LAYOUT_STORE: &str = ".makepad-layouts.txt";

/// A name the person gave a widget. `from` is what the tree calls it today
/// (empty for an anonymous widget), `to` what they want it called.
#[derive(Clone, Debug)]
pub struct TweakRename {
    pub reference: String,
    pub from: String,
    pub to: String,
}

/// Read the requested names back.
fn name_store_load() -> Vec<TweakRename> {
    let Ok(body) = std::fs::read_to_string(NAME_STORE) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in body.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        out.push(TweakRename {
            reference: cols[0].to_string(),
            from: note_store_unescape(cols[1]),
            to: note_store_unescape(cols[2]),
        });
    }
    out
}

fn name_store_save(renames: &[TweakRename]) {
    if renames.is_empty() {
        let _ = std::fs::remove_file(NAME_STORE);
        return;
    }
    let mut out = String::from(
        "# makepad widget names — typed in the Shift+F10 Props tab, one per line\n\
         # reference\tcurrent name\twanted name\n",
    );
    for rename in renames {
        out.push_str(&format!(
            "{}\t{}\t{}\n",
            rename.reference,
            note_store_escape(&rename.from),
            note_store_escape(&rename.to)
        ));
    }
    if let Err(error) = std::fs::write(NAME_STORE, out) {
        log!("TWEAK name store write failed: {error}");
    }
}

/// One layout conversion asked for: which widget, what it is, what it
/// should become (`grid` or `flex`).
#[derive(Clone, Debug)]
pub struct TweakConvert {
    pub reference: String,
    pub from: String,
    pub to: String,
}

fn layout_store_load() -> Vec<TweakConvert> {
    let Ok(body) = std::fs::read_to_string(LAYOUT_STORE) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in body.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        out.push(TweakConvert {
            reference: cols[0].to_string(),
            from: cols[1].to_string(),
            to: cols[2].to_string(),
        });
    }
    out
}

fn layout_store_save(converts: &[TweakConvert]) {
    if converts.is_empty() {
        let _ = std::fs::remove_file(LAYOUT_STORE);
        return;
    }
    let mut out = String::from(
        "# makepad layout conversions \u{2014} asked for in the Shift+F10 Props tab, one per line\n\
         # reference\tcurrent type\twanted layout\n",
    );
    for convert in converts {
        out.push_str(&format!("{}\t{}\t{}\n", convert.reference, convert.from, convert.to));
    }
    if let Err(error) = std::fs::write(LAYOUT_STORE, out) {
        log!("TWEAK layout store write failed: {error}");
    }
}

fn note_store_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

fn note_store_unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Put a mentioned widget's path into the note text, right after the `@`
/// that armed the pick. With no `@` (the arming character was deleted mid
/// pick) it is appended, so a click is never silently lost.
fn insert_mention(text: &str, path: &str) -> String {
    match text.rfind('@') {
        Some(at) => {
            let mut out = String::with_capacity(text.len() + path.len());
            out.push_str(&text[..=at]);
            out.push_str(path);
            out.push_str(&text[at + 1..]);
            out
        }
        None => {
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with(char::is_whitespace) {
                out.push(' ');
            }
            out.push('@');
            out.push_str(path);
            out
        }
    }
}

/// `target` written relative to the noted widget, in URL notation — the one
/// notation the whole scheme uses:
///
/// * `/dock/tOverview/View/Label_1` is absolute, from the root,
/// * `./child` is inside the widget the note is on,
/// * `../Label_2` is a SIBLING (up to the parent, then down), and
/// * `../../Button` is one level further out.
///
/// This is what the person sees in the card, because `../Label_2` says "the
/// one next to this", which no absolute path can say however short it is.
/// What leaves the app — the clipboard, `/tweak/state` — is always absolute:
/// a reference read somewhere else has no "here" to be relative to.
///
/// `None` when the two share no root, or when the target is a bare ancestor
/// (`../` alone names it but says nothing about WHAT it is).
fn relative_path(base: &str, target: &str) -> Option<String> {
    let split = |p: &str| -> Vec<String> {
        p.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect()
    };
    let base = split(base);
    let target = split(target);
    let shared = base
        .iter()
        .zip(target.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if shared == 0 {
        return None; // different trees: relative would be a lie
    }
    let down = target[shared..].join("/");
    if down.is_empty() {
        return None;
    }
    let ups = base.len() - shared;
    let mut out = if ups == 0 {
        "./".to_string()
    } else {
        "../".repeat(ups)
    };
    out.push_str(&down);
    Some(out)
}

/// The absolute path a mention names. `./` and `../` are read against the
/// noted widget (see [`relative_path`]); anything else is already absolute.
fn absolute_mention(base: &str, mention: &str) -> String {
    if !mention.starts_with("./") && !mention.starts_with("../") {
        // Already absolute — give it the leading slash if it was written
        // without one.
        return if mention.starts_with('/') {
            mention.to_string()
        } else {
            format!("/{mention}")
        };
    }
    let mut rest = mention;
    let mut ups = 0;
    while let Some(tail) = rest.strip_prefix("../") {
        ups += 1;
        rest = tail;
    }
    let rest = rest.strip_prefix("./").unwrap_or(rest);
    let base: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    let keep = base.len().saturating_sub(ups);
    let mut out: Vec<&str> = base[..keep].to_vec();
    if !rest.is_empty() {
        out.extend(rest.split('/').filter(|s| !s.is_empty()));
    }
    format!("/{}", out.join("/"))
}

/// Every widget a note points at: each `@` in its text followed by a run of
/// path characters, resolved to absolute against the note's own widget.
/// Derived rather than stored, so it survives a hand-edited note store and
/// can never drift from what the text actually says.
fn note_mentions(base: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'@' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut end = start;
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric()
                || matches!(bytes[end], b'.' | b'_' | b'-' | b'/'))
        {
            end += 1;
        }
        // A trailing dot is sentence punctuation, and a trailing slash names
        // nothing further — neither is part of the reference.
        while end > start && matches!(bytes[end - 1], b'.' | b'/') {
            end -= 1;
        }
        if end > start {
            let path = absolute_mention(base, &text[start..end]);
            if !out.contains(&path) {
                out.push(path);
            }
        }
        i = end.max(start);
    }
    out
}

/// Read the pinned notes back. Anything malformed is skipped rather than
/// fatal: the file is meant to be hand-editable.
fn note_store_load() -> Vec<TweakNote> {
    let Ok(body) = std::fs::read_to_string(NOTE_STORE) else {
        return Vec::new();
    };
    note_store_parse(&body)
}

/// The parser on its own, so both shapes of the file can be tested without
/// one on disk.
fn note_store_parse(body: &str) -> Vec<TweakNote> {
    let mut out = Vec::new();
    for line in body.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        // Two shapes are read. The current one is `path\tnotes\trules`.
        // The old one carried the card's geometry in columns 1-4 with the
        // text in column 5, and files written by it still exist: take the
        // text from where it was and drop the geometry, which describes a
        // card that no longer exists.
        let (path, text, rules) = match cols.len() {
            0 | 1 => continue,
            2 => (cols[0], note_store_unescape(cols[1]), String::new()),
            3..=5 => (
                cols[0],
                note_store_unescape(cols[1]),
                note_store_unescape(cols[2]),
            ),
            _ => (cols[0], note_store_unescape(cols[5]), String::new()),
        };
        out.push(TweakNote {
            path: path.to_string(),
            text,
            rules,
            sent: 0,
        });
    }
    out
}

/// Write every pinned note out. Called on each pin toggle and on each text
/// commit of a pinned note — the file is tiny and the write is rare.
fn note_store_save(notes: &[TweakNote]) {
    // Pinning is gone: anything written about a widget is worth keeping, so
    // every record that says something is saved and empty ones are not.
    let pinned: Vec<&TweakNote> = notes
        .iter()
        .filter(|n| !n.text.trim().is_empty() || !n.rules.trim().is_empty())
        .collect();
    if pinned.is_empty() {
        // Nothing pinned any more: take the file away rather than leave an
        // empty one lying beside the app.
        let _ = std::fs::remove_file(NOTE_STORE);
        return;
    }
    let mut out = String::from(
        "# makepad tweak notes \u{2014} written in the Shift+F10 Spec tab, one per line\n\
         # path\\tnotes\\trules (\\\\n for newlines)\n",
    );
    for note in pinned {
        out.push_str(&format!(
            "{}\t{}\t{}\n",
            note.path,
            note_store_escape(&note.text),
            note_store_escape(&note.rules)
        ));
    }
    if let Err(error) = std::fs::write(NOTE_STORE, out) {
        log!("TWEAK note store write failed: {error}");
    }
}

/// One undoable edit gesture. Value: a contiguous run of applies to one
/// prop (a scrub down->up, a text commit, chevron steps) — old is the
/// pre-gesture value, new the latest. Reset: a double-click reset, with
/// the pruned ledger entries so undo can restore them.
#[derive(Clone, Debug)]
enum UndoStep {
    Value {
        path: String,
        prop: String,
        old: String,
        new: String,
        /// seq of the gesture's first ledger entry: undo removes every
        /// entry for (path, prop) from here on — as if never touched.
        seq_start: u64,
    },
    Reset {
        path: String,
        prop: String,
        removed: Vec<TweakDiffEntry>,
    },
}

/// One freehand annotation stroke, in window-local points, tagged with the
/// widget paths it touches.
#[derive(Clone, Debug, Default)]
pub struct TweakStroke {
    pub window_id: usize,
    pub points: Vec<(f64, f64)>,
    pub widgets: Vec<String>,
}

/// The tweak session's state. Crate-visible only so the theme edit path can
/// name its undo sink's type; the fields stay this module's own.
#[derive(Default)]
pub(crate) struct TweakSession {
    /// Guards against N windows toggling N times on one Shift+F10 event.
    toggle_event_id: u64,
    /// The pinned selection (click pins; remote applies re-pin by path).
    pinned: Option<TweakPick>,
    hover: Option<TweakPick>,
    /// The "isolated" toggle is OFF: an "all" edit reaches the whole app
    /// even while a branch is isolated. Stored inverted so the derived
    /// default is the confined one — with the rest of the app covered,
    /// editing it unseen is the surprising answer, not the safe one.
    scope_unconfined: bool,
    /// The widget isolation is locked onto (0 = not isolating), mirrored
    /// from the panel so the apply path can confine a fan-out to it.
    isolate_uid: u64,
    /// The selection is LOCKED: the overlay has handed the mouse back to the
    /// app. Nothing on the canvas hovers or picks, so buttons, sliders and
    /// tabs work under the pointer as they always would, and what is
    /// selected stays selected for the panel to keep editing. False — the
    /// overlay picking — is the ordinary state.
    selection_locked: bool,
    /// Live pointer position (window abs), for hover-revealed handles.
    pointer_abs: Vec2d,
    diff: Vec<TweakDiffEntry>,
    next_seq: u64,
    strokes: Vec<TweakStroke>,
    /// A stroke being drawn right now (annotate mode, mouse held down).
    live_stroke: Option<TweakStroke>,
    /// Annotate mode: drag draws instead of picking.
    annotate: bool,
    /// True once the user drew anything — `/tweak/final` then includes a
    /// composited screenshot so the AI sees what the drawings mean.
    drew: bool,
    /// A mouse-down we swallowed; swallow the matching up too.
    down_consumed: bool,
    /// Eyedropper armed for this prop: the next press in the body samples
    /// the pixel under it instead of picking.
    eyedrop: Option<String>,
    /// Shader fns applied live from the source view: (uid, layer, fn name)
    /// → the text as applied, shown in place of the file's version.
    fn_overrides: HashMap<(u64, String, String), String>,
    /// What the panel says under the prompt: "sent … waiting", "applied",
    /// or an error — the send must be visible, and so must the landing.
    vibe_status: String,
    /// Bumped by every fn apply that did NOT come from the source view (the
    /// AI, /tweak/apply): the view re-reads its text for those only —
    /// otherwise a person's half-typed edit would be replaced under them.
    fn_override_gen: u64,
    /// A remote undo (true) / redo (false) request, consumed by the
    /// tweaker's event loop (Cmd+Z / Shift+Cmd+Z by the bridge).
    undo_redo: Option<bool>,
    /// A remote pulse request: a theme colour name (or #rrggbbaa) to pulse
    /// app-wide until an empty request clears it; consumed by the tweaker.
    pulse_req: Option<String>,
    /// The Spec tab's three field weights, dragged by the splitters between
    /// them. See [`spec_weight`].
    spec_weights: [f64; 3],
    /// The open colour popover's window rect, for /tweak/state.
    popup: Option<Rect>,
    /// A remote lock on the pulse mix (deterministic grabs): the pulse
    /// holds that tone instead of animating. None animates.
    pulse_lock: Option<f32>,
    /// The live theme pulse, shared by the tweaker's tick and the
    /// post-draw hook (taken out of the lock while either runs).
    pulse: Option<PulseState>,
    /// Theme colour edits in flight: name -> the original colour and the
    /// value every use of it now shows (re-applied after each draw).
    theme_overrides: Vec<(String, PulseState)>,
    /// A remote theme edit (`op=theme name= value=`), consumed by the tweaker.
    theme_req: Option<(String, String)>,
    /// Remote pose lock for the state swatches: Some(0..1) freezes every
    /// track at that mix (deterministic grabs), None animates.
    states_lock: Option<f64>,
    /// The pinned widget's animator groups, for /tweak/state.
    state_names: Vec<String>,
    /// Edit scope: false = "this" (specialise this instance), true = "all"
    /// (every live widget of the type; the ledger names the type's site).
    scope_all: bool,
    /// A prompt is out and unanswered (path, layer).
    vibe_pending: Option<(String, String)>,
    /// A sample in flight: (probe id, prop). Answered from the next frame.
    eyedrop_probe: Option<(u64, String)>,
    /// The press went to a navigation widget (tab, fold, dropdown) and was
    /// NOT consumed; its release must flow through too.
    pass_up: bool,
    /// The press went to the tweaker's own popover (colour picker) while it
    /// held the edit; the release must reach it too or its buttons never
    /// complete a click.
    hold_up: bool,
    /// The widget under the pointer at the last pick (the deepest hit, not
    /// where the pin climbed to): click-to-climb continues only from the
    /// same widget — a press on a different widget inside the pinned
    /// container picks that widget.
    climb_origin: u64,
    /// Sidebar width in points (0 = use the default).
    sidebar_width: f64,
    /// The panel band and the note card, in window coordinates, as of the
    /// last draw — the chrome that OWNS the pointer where it sits. Read by
    /// [`panel_owns_pointer`] from outside the widget.
    ///
    /// Two fields, not one list: the card is published by the overlay draw
    /// and the band by the sidebar draw, which runs after it in the same
    /// frame. Sharing a list meant whichever wrote second erased the other,
    /// and it was always the card that lost — so a note dragged up against
    /// the top of the window went back to being unclickable.
    chrome_band: Option<Rect>,
    /// The chrome that FLOATS over the app: the note card and the extrusion
    /// readout. Published by the overlay draw, which is why it cannot share
    /// a slot with the band — the sidebar draws after it.
    chrome_float: Vec<Rect>,
    /// The on-canvas selection outline hides until this time: an edit was
    /// applied within the last beat, so the widget must be seen exactly as
    /// it renders. Extended by every apply (sidebar or remote).
    suppress_until: f64,
    /// A sidebar interaction owns the canvas right now (field focused,
    /// color popover open): the outline stays hidden for its duration.
    edit_hold: bool,
    /// Throttle for sidebar TWEAK log lines: (path, prop, time) of the last
    /// emitted line, so a typing/scrubbing burst logs once per pause.
    last_sidebar_log: Option<(String, String, f64)>,
    /// Vibecode prompts sent this session: (sel path, layer, prompt).
    /// Surfaced in /tweak/state as the agent's work queue.
    vibes: Vec<(String, String, String, String)>,
    /// A press in the EXPLODED view whose pick is waiting for the release:
    /// the position it went down at, or `None` when no press is held. A
    /// release further than [`CLIMB_SLOP`] from it was an orbit, and an
    /// orbit selects nothing.
    press_pick: Option<Vec2d>,
    /// Names the person asked for on the identity row. Reported in
    /// `/tweak/state` for the AI to carry out in the source — the running app
    /// cannot rename its own widgets without lying about them — and kept in
    /// [`NAME_STORE`] so the ask survives until it is done.
    renames: Vec<TweakRename>,
    /// The name store has been read back: once per process.
    renames_loaded: bool,
    /// Layout conversions asked for on the Props tab (`make grid`, `make
    /// flex`), reported in `/tweak/state` and kept in [`LAYOUT_STORE`].
    converts: Vec<TweakConvert>,
    converts_loaded: bool,
    /// Messages written and queued but not yet sent: (note key, text). They
    /// wake nobody until a Ctrl+Enter releases the batch.
    outbox: Vec<Outgoing>,
    /// The prompt strip's text. ONE box for the whole panel, not one per
    /// widget: a send attributes it to whatever is selected at that moment,
    /// which is what the `TWEAK ask #N <path>` line already recorded. In
    /// memory only -- a half-typed instruction is not worth a file.
    prompt: String,
    /// Everything ever sent or queued from the strip, oldest first. Sending
    /// CLEARS the box, so this is where a message goes to stay recallable.
    prompt_history: Vec<String>,
    /// How far back through `prompt_history` Up has walked;
    /// `prompt_history.len()` is the empty draft being typed now.
    prompt_at: usize,
    /// What the strip says under the box: "2 queued", "sent", or why not.
    prompt_status: String,
    /// The rules that stand over the whole app, read once from their file.
    app_rules: String,
    app_rules_loaded: bool,
    /// A note is mid-@mention: an `@` was just typed into the open card, so
    /// the next click in the app names a widget INTO the note instead of
    /// changing the selection. The hover outline turns amber to say so.
    mention: bool,
    /// Which box the `@` was typed into, because that is where the name
    /// lands: the prompt strip is present on every tab, so the tab cannot
    /// say.
    mention_from_prompt: bool,
    /// Note cards, keyed by widget path (one per widget).
    notes: Vec<TweakNote>,
    /// The pinned notes have been read back from the store: once per process,
    /// at the first note the session touches.
    notes_loaded: bool,
    /// The undo stack over edit gestures (Cmd+Z / Cmd+Shift+Z).
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
    /// True while the current gesture may still merge into the top undo
    /// step (closed by HoldOff so two scrubs never merge).
    undo_open: bool,
    /// Bumped by every applied change (any origin) and by resets/clears:
    /// the sidebar rebuilds its rows when it sees a new generation.
    apply_gen: u64,
}

fn session() -> &'static Mutex<TweakSession> {
    static S: OnceLock<Mutex<TweakSession>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(TweakSession::default()))
}

impl TweakSession {
    /// Pull the pinned notes in, once per process. Anything already open in
    /// this session wins over the stored copy — the human is looking at it.
    fn load_notes(&mut self) {
        if self.notes_loaded {
            return;
        }
        self.notes_loaded = true;
        let stored = note_store_load();
        if !stored.is_empty() {
            log!("TWEAK note store: {} pinned note(s) from {NOTE_STORE}", stored.len());
        }
        for note in stored {
            if !self.notes.iter().any(|n| n.path == note.path) {
                self.notes.push(note);
            }
        }
    }

    /// Pull the app-wide rules in, once per process. Same shape as
    /// `load_notes`: the file is read lazily, because most sessions never
    /// open the Spec tab at all.
    fn load_app_rules(&mut self) {
        if self.app_rules_loaded {
            return;
        }
        self.app_rules_loaded = true;
        self.app_rules = app_rules_load();
        if !self.app_rules.trim().is_empty() {
            log!("TWEAK rules store: app rules read from {RULES_STORE}");
        }
    }

    /// Pull the requested names in, once per process.
    fn load_renames(&mut self) {
        if self.renames_loaded {
            return;
        }
        self.renames_loaded = true;
        let stored = name_store_load();
        if !stored.is_empty() {
            log!("TWEAK name store: {} wanted name(s) from {NAME_STORE}", stored.len());
        }
        for rename in stored {
            if !self.renames.iter().any(|r| r.reference == rename.reference) {
                self.renames.push(rename);
            }
        }
    }

    fn load_converts(&mut self) {
        if self.converts_loaded {
            return;
        }
        self.converts_loaded = true;
        let stored = layout_store_load();
        if !stored.is_empty() {
            log!("TWEAK layout store: {} conversion(s) asked for, from {LAYOUT_STORE}", stored.len());
        }
        for convert in stored {
            if !self.converts.iter().any(|c| c.reference == convert.reference) {
                self.converts.push(convert);
            }
        }
    }
}

/// The pinned selection's outline rect: the widget, held off by
/// [`SELECTION_RING_OUTSET`] so the edge pixels being judged stay clean.
fn selection_ring(rect: Rect) -> Rect {
    Rect {
        pos: dvec2(rect.pos.x - SELECTION_RING_OUTSET, rect.pos.y - SELECTION_RING_OUTSET),
        size: dvec2(
            rect.size.x + SELECTION_RING_OUTSET * 2.0,
            rect.size.y + SELECTION_RING_OUTSET * 2.0,
        ),
    }
}

/// How far a second click may land from the first and still count as the
/// same click — the gesture that climbs to the parent. A hand does not put
/// the pointer back on the same pixel.
const CLIMB_SLOP: f64 = 3.0;

const DEFAULT_SIDEBAR_WIDTH: f64 = 280.0;
const SPLITTER_WIDTH: f64 = 5.0;

/// How long the footer says "path copied" before showing the path again.
const FOOTER_COPIED_LINGER: f64 = 1.2;

/// How long the selection outline stays quiet after the last applied edit.
const SUPPRESS_LINGER: f64 = 0.5;

/// A thick red frame round the whole window while the bridge is driving.
/// The one thing a person watching a scripted run needs to know is that
/// the pointer and the keyboard are spoken for, and a frame the size of the
/// window says it from across the room. Lit by the bridge itself for a few
/// seconds after any injected input, or held up by `/handsoff?on=1`.
///
/// Returns whether it drew, so the caller can keep the frames coming until
/// it has gone quiet -- the frame must disappear on its own, not wait for
/// the next thing that happens to redraw.
fn draw_hands_off_frame(cx: &mut Cx2d, outline: &mut DrawTweakOutline) -> bool {
    if !makepad_platform::remote::hands_off_active() {
        return false;
    }
    let size = cx.current_pass_size();
    const T: f64 = 6.0;
    outline.fill_color = vec4(0.93, 0.13, 0.13, 0.95);
    for rect in [
        Rect { pos: dvec2(0.0, 0.0), size: dvec2(size.x, T) },
        Rect { pos: dvec2(0.0, size.y - T), size: dvec2(size.x, T) },
        Rect { pos: dvec2(0.0, 0.0), size: dvec2(T, size.y) },
        Rect { pos: dvec2(size.x - T, 0.0), size: dvec2(T, size.y) },
    ] {
        outline.draw_abs(cx, rect);
    }
    true
}

fn sidebar_width() -> f64 {
    let width = session().lock().unwrap().sidebar_width;
    if width <= 0.0 {
        DEFAULT_SIDEBAR_WIDTH
    } else {
        width
    }
}

/// Turn the overlay on/off and force the full-app redraw that makes the
/// change visible everywhere.
pub fn set_tweak_on(cx: &mut Cx, on: bool) {
    let was = TWEAK_ON.swap(on, Ordering::Relaxed);
    if was != on {
        if !on {
            let mut s = session().lock().unwrap();
            s.hover = None;
            s.down_consumed = false;
            s.live_stroke = None;
            drop(s);
            // Shift+F10 closes the whole design surface: the exploded view goes
            // with the panel (deferred toggle — performed pre-dispatch at
            // the next event), the marks and the flat band with it, so
            // the app is never left tilted without its panel.
            if cx.sploded_will_be_active() {
                cx.sploded_toggle();
            }
            cx.sploded_set_marks(None, None);
            cx.sploded_set_selected(false);
            {
                let mut s = session().lock().unwrap();
                s.chrome_band = None;
                s.chrome_float.clear();
            }
            cx.sploded_set_flat_band(None);
            cx.sploded_set_flat_rects(Vec::new());
        }
        log!("TWEAK mode {}", if on { "on" } else { "off" });
        cx.redraw_all();
    }
}

// ---------------------------------------------------------------------------
// pick resolution — the reflection walk's spine is the ordinary widget
// hierarchy: deepest visible widget whose clipped rect contains the point,
// later siblings (drawn on top) winning. `View.design_mode` is the revived
// container seam: a design-mode container is transparent to picking — its
// children resolve, it never does.
// ---------------------------------------------------------------------------

fn pick_candidate(
    cx: &Cx,
    widget: &WidgetRef,
    abs: Vec2d,
    attached: &HashSet<DrawListId>,
) -> Option<Rect> {
    if !widget.visible() {
        return None;
    }
    // Only what is on screen: a hidden Dock page keeps its retained draw
    // list and every rect in it, and clicking "through" to those was the
    // stale-tab bug.
    if !widget.area().is_attached(cx, attached) {
        return None;
    }
    // All instances, not the first: a text run's area is its glyph run,
    // and the first glyph alone is not a paragraph.
    let rect = widget.area().clipped_rect_union(cx);
    if rect.size.x <= 0.0 || rect.size.y <= 0.0 || !rect.contains(abs) {
        return None;
    }
    Some(rect)
}

/// The draw lists on screen, over every pass: a hidden page's list hangs
/// off no pass at all, so the union is exactly "visible". Computed at EVENT
/// time (between frames, when every list has linked into its parent) and
/// cached for the overlay draw, which runs mid-frame — while a list is
/// still open it has not linked into its parent yet, so a walk taken then
/// missed every page inside the dock (the "only tabs light up" bug).
fn attached_lists_of(cx: &Cx, _widget: &WidgetRef) -> HashSet<DrawListId> {
    let mut out = HashSet::new();
    for pass_id in cx.passes.id_iter() {
        out.extend(cx.attached_draw_lists(pass_id));
    }
    *attached_cache().lock().unwrap() = out.clone();
    out
}

fn attached_cache() -> &'static Mutex<HashSet<DrawListId>> {
    static CACHE: OnceLock<Mutex<HashSet<DrawListId>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

/// A widget's on-screen rect this frame: empty when it is not drawn (its
/// page is hidden), whatever its retained draw list still holds.
/// A widget's on-screen rect this frame: empty when it is not drawn (its
/// page is hidden), whatever its retained draw list still holds. Draw-time
/// variant: the lists still open right now count as attached, because a
/// list links into its parent only when it ends.
/// A widget's on-screen rect this frame: empty when it is not drawn (its
/// page is hidden), whatever its retained draw list still holds. Uses the
/// attachment set the last EVENT computed (see `attached_lists_of`): the
/// overlay draws mid-frame, when open lists are not yet linked.
fn live_rect(cx: &Cx2d, widget: &WidgetRef) -> Rect {
    let attached = attached_cache().lock().unwrap().clone();
    if !attached.is_empty() && !widget.area().is_attached(cx, &attached) {
        return Rect::default();
    }
    // Attached = on screen; read the geometry even if a retained list left
    // the area one redraw stale (the tab strip does).
    widget.area().clipped_rect_union_attached(cx)
}


/// Navigation-class widgets keep working under the pick: "since tabs show
/// whole new chunks of clickable UI", a plain click on a tab, fold button,
/// dropdown opener or stack-navigation control both PINS it and performs
/// its normal action, so every corner of the app stays reachable while
/// tweaking. Fold headers and expandable panels count only for their
/// `header` subtree — a button in a fold's body is content, not navigation.
fn is_navigation_pick(cx: &mut Cx, uid: WidgetUid) -> bool {
    const NAV: &[&str] = &["Tab", "TabBar", "FoldButton", "DropDown", "StackNavigation"];
    const HEADER_NAV: &[&str] = &["FoldHeader", "ExpandablePanel"];
    let mut cur = Some(uid);
    let mut child_name: Option<LiveId> = None;
    for _ in 0..16 {
        let Some(u) = cur else { break };
        let widget = cx.widget_tree().widget(u);
        if widget.is_empty() {
            break;
        }
        let ty = widget
            .widget_type_id()
            .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
            .map(live_id_token)
            .unwrap_or_default();
        if NAV.contains(&ty.as_str()) {
            return true;
        }
        if HEADER_NAV.contains(&ty.as_str()) {
            return child_name == Some(live_id!(header));
        }
        child_name = cx.widget_tree().name_of(u);
        cur = cx.widget_tree().parent_of(u);
    }
    false
}

/// Bring a widget into view: open whatever is holding it shut.
///
/// A tree row can name something on a dock tab that is not selected, inside a
/// fold that is closed, on a page that is not showing — in which case
/// selecting it outlines nothing and the panel fills with a widget the person
/// cannot see. So walk the ancestors and ask each container that hides its
/// children to show the branch the target is on: the Dock selects the tab,
/// the FoldHeader opens, the PageFlip flips.
///
/// Top-down, because opening an outer container is what makes the inner ones
/// exist to be opened.
fn reveal_widget(cx: &mut Cx, uid: u64) {
    // The chain from the target up, each step remembering which child it came
    // through — that child IS the tab / page to switch to.
    let mut chain: Vec<(WidgetUid, LiveId)> = Vec::new();
    let mut cur = WidgetUid(uid);
    for _ in 0..64 {
        let Some(parent) = cx.widget_tree().parent_of(cur) else { break };
        let Some(name) = cx.widget_tree().name_of(cur) else { break };
        chain.push((parent, name));
        cur = parent;
    }
    let mut opened = 0;
    for (parent, child) in chain.into_iter().rev() {
        let widget = cx.widget_tree().widget(parent);
        if widget.is_empty() {
            continue;
        }
        let ty = widget
            .widget_type_id()
            .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
            .map(live_id_token)
            .unwrap_or_default();
        match ty.as_str() {
            "Dock" => {
                widget.as_dock().select_tab(cx, child);
                opened += 1;
            }
            "FoldHeader" => {
                let fold = widget.as_fold_header();
                if !fold.is_open(cx) {
                    fold.set_is_open(cx, true, Animate::No);
                    opened += 1;
                }
            }
            "PageFlip" => {
                widget.as_page_flip().set_active_page(cx, child);
                opened += 1;
            }
            _ => {}
        }
    }
    if opened > 0 {
        cx.redraw_all();
    }
}

fn is_design_transparent(widget: &WidgetRef) -> bool {
    widget
        .borrow::<View>()
        .map_or(false, |view| view.design_mode())
}

fn walk_pick(
    cx: &Cx,
    widget: &WidgetRef,
    abs: Vec2d,
    depth: usize,
    max_level: Option<usize>,
    attached: &HashSet<DrawListId>,
    best: &mut Option<(WidgetRef, Rect, usize)>,
) {
    if !widget.visible() {
        return;
    }
    // Exploded view: the cursor is on ONE plane. A widget nested deeper than
    // that plane sits on another sheet, however its 2D rect overlaps the
    // un-projected point — that is what makes a covered parent selectable.
    let on_deeper_plane = max_level.is_some_and(|max| {
        cx.sploded_depth_of(widget.widget_uid().0)
            .is_some_and(|level| level > max)
    });
    if !on_deeper_plane && !is_design_transparent(widget) {
        if let Some(rect) = pick_candidate(cx, widget, abs, attached) {
            let take = match best {
                // Deeper wins; equal depth: later in draw order wins.
                Some((_, _, best_depth)) => depth >= *best_depth,
                None => true,
            };
            if take {
                *best = Some((widget.clone(), rect, depth));
            }
        }
    }
    widget.children(&mut |_id, child| {
        walk_pick(cx, &child, abs, depth + 1, max_level, attached, best);
    });
}

/// The spacing bands: when the picked widget is a container and the point
/// missed all of its children, say whether the point sits in the container's
/// padding ring or in a child's margin ring.
fn resolve_band(cx: &mut Cx, widget: &WidgetRef, rect: Rect, abs: Vec2d) -> Option<String> {
    let mut child_hit = false;
    let mut margin_of = None;
    widget.children(&mut |id, child| {
        if !child.visible() {
            return;
        }
        let child_rect = child.area().clipped_rect_union(cx);
        if child_rect.size.x <= 0.0 || child_rect.size.y <= 0.0 {
            return;
        }
        if child_rect.contains(abs) {
            child_hit = true;
            return;
        }
        let margin = child.walk(cx).margin;
        let expanded = Rect {
            pos: dvec2(
                child_rect.pos.x - margin.left,
                child_rect.pos.y - margin.top,
            ),
            size: dvec2(
                child_rect.size.x + margin.left + margin.right,
                child_rect.size.y + margin.top + margin.bottom,
            ),
        };
        if margin_of.is_none() && expanded.contains(abs) {
            margin_of = Some(format!("margin:{}", live_id_token(id)));
        }
    });
    if child_hit {
        return None;
    }
    if let Some(margin) = margin_of {
        return Some(margin);
    }
    // The gap between two adjacent siblings (flow spacing) is a first-class
    // target too: name both neighbours.
    let mut above: Option<(f64, LiveId)> = None;
    let mut below: Option<(f64, LiveId)> = None;
    let mut left: Option<(f64, LiveId)> = None;
    let mut right: Option<(f64, LiveId)> = None;
    widget.children(&mut |id, child| {
        if !child.visible() {
            return;
        }
        let r = child.area().clipped_rect_union(cx);
        if r.size.x <= 0.0 || r.size.y <= 0.0 {
            return;
        }
        let x_overlaps = abs.x >= r.pos.x && abs.x <= r.pos.x + r.size.x;
        let y_overlaps = abs.y >= r.pos.y && abs.y <= r.pos.y + r.size.y;
        if x_overlaps {
            let bottom = r.pos.y + r.size.y;
            if bottom <= abs.y && above.as_ref().map_or(true, |(edge, _)| bottom > *edge) {
                above = Some((bottom, id));
            }
            if r.pos.y >= abs.y && below.as_ref().map_or(true, |(edge, _)| r.pos.y < *edge) {
                below = Some((r.pos.y, id));
            }
        }
        if y_overlaps {
            let edge = r.pos.x + r.size.x;
            if edge <= abs.x && left.as_ref().map_or(true, |(e, _)| edge > *e) {
                left = Some((edge, id));
            }
            if r.pos.x >= abs.x && right.as_ref().map_or(true, |(e, _)| r.pos.x < *e) {
                right = Some((r.pos.x, id));
            }
        }
    });
    if let (Some((_, a)), Some((_, b))) = (&above, &below) {
        return Some(format!("gap:{}~{}", live_id_token(*a), live_id_token(*b)));
    }
    if let (Some((_, a)), Some((_, b))) = (&left, &right) {
        return Some(format!("gap:{}~{}", live_id_token(*a), live_id_token(*b)));
    }
    if let Some(view) = widget.borrow::<View>() {
        let padding = view.layout.padding;
        let content = Rect {
            pos: dvec2(rect.pos.x + padding.left, rect.pos.y + padding.top),
            size: dvec2(
                rect.size.x - padding.left - padding.right,
                rect.size.y - padding.top - padding.bottom,
            ),
        };
        if (padding.left != 0.0
            || padding.right != 0.0
            || padding.top != 0.0
            || padding.bottom != 0.0)
            && !content.contains(abs)
        {
            return Some("padding".to_string());
        }
    }
    None
}

fn resolve_pick(
    cx: &mut Cx,
    root: &WidgetRef,
    abs: Vec2d,
    window_id: usize,
) -> Option<TweakPick> {
    let mut best = None;
    // Exploded: the router already re-addressed `abs` onto the plane the ray
    // hit; a miss (off the deck) picks nothing.
    let max_level = if cx.sploded_active() {
        Some(cx.sploded_hit_level()?)
    } else {
        None
    };
    let attached = attached_lists_of(cx, root);
    // Start below the root (the window body itself is chrome, not content).
    root.children(&mut |_id, child| {
        walk_pick(cx, &child, abs, 0, max_level, &attached, &mut best);
    });
    let (widget, rect, _depth) = best?;
    let uid = widget.widget_uid();
    let level = cx.sploded_depth_of(uid.0).unwrap_or(0);
    let path_ids = cx.widget_tree().path_to(uid);
    let path = if path_ids.is_empty() {
        format!("uid:{}", uid.0)
    } else {
        path_ids
            .iter()
            .map(|id| live_id_token(*id))
            .collect::<Vec<_>>()
            .join(".")
    };
    let ty = widget
        .widget_type_id()
        .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
        .map(live_id_token)
        .unwrap_or_else(|| "-".to_string());
    let band = resolve_band(cx, &widget, rect, abs);
    Some(TweakPick {
        uid: uid.0,
        path,
        ty,
        rect,
        window_id,
        band,
        level,
    })
}

/// A TweakPick for a KNOWN widget (the climb's steps), same fields as a
/// resolved one.
fn pick_of_widget(
    cx: &mut Cx,
    widget: &WidgetRef,
    abs: Vec2d,
    window_id: usize,
) -> Option<TweakPick> {
    let uid = widget.widget_uid();
    let rect = widget.area().clipped_rect_union(cx);
    if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
        return None;
    }
    let level = cx.sploded_depth_of(uid.0).unwrap_or(0);
    let path_ids = cx.widget_tree().path_to(uid);
    let path = if path_ids.is_empty() {
        format!("uid:{}", uid.0)
    } else {
        path_ids
            .iter()
            .map(|id| live_id_token(*id))
            .collect::<Vec<_>>()
            .join(".")
    };
    let ty = widget
        .widget_type_id()
        .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
        .map(live_id_token)
        .unwrap_or_else(|| "-".to_string());
    let band = resolve_band(cx, widget, rect, abs);
    Some(TweakPick { uid: uid.0, path, ty, rect, window_id, band, level })
}

/// The registered type name of a widget, `View`, `Grid`, `Label`.
fn type_name_of(cx: &mut Cx, uid: u64) -> Option<String> {
    let widget = cx.widget_tree().widget(WidgetUid(uid));
    if widget.is_empty() {
        return None;
    }
    widget
        .widget_type_id()
        .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
        .map(live_id_token)
}

/// Is `ancestor` on `uid`'s parent chain?
fn is_ancestor_of(cx: &mut Cx, ancestor: u64, uid: u64) -> bool {
    let mut cur = cx.widget_tree().parent_of(WidgetUid(uid));
    for _ in 0..64 {
        match cur {
            Some(u) if u.0 == ancestor => return true,
            Some(u) => cur = cx.widget_tree().parent_of(u),
            None => return false,
        }
    }
    false
}

/// The pin's next ancestor worth pinning: skips design-transparent views and
/// zero-rect wrappers; `None` at the top (the caller wraps to the deepest).
fn ancestor_pick(
    cx: &mut Cx,
    pin: &TweakPick,
    abs: Vec2d,
    window_id: usize,
) -> Option<TweakPick> {
    let mut cur = cx.widget_tree().parent_of(WidgetUid(pin.uid));
    for _ in 0..64 {
        let u = cur?;
        // The window and above are chrome, not content — and the window is
        // mutably borrowed mid-dispatch, so it must not even be touched
        // (tree lookups only, no widget borrow, before deciding).
        let above = cx.widget_tree().parent_of(u)?;
        if cx.widget_tree().parent_of(above).is_none() {
            return None;
        }
        let widget = cx.widget_tree().widget(u);
        if widget.is_empty() {
            return None;
        }
        if !is_design_transparent(&widget) {
            if let Some(pick) = pick_of_widget(cx, &widget, abs, window_id) {
                // An ancestor whose area misses the click (a splitter whose
                // rect is only its grab bar) would throw the brackets to a
                // far-away sliver — climb past it.
                if pick.rect.contains(abs) {
                    return Some(pick);
                }
            }
        }
        cur = cx.widget_tree().parent_of(u);
    }
    None
}

// ---------------------------------------------------------------------------
// the Window seam — swallow pointer events before ordinary dispatch while
// the overlay is on, so picking can never activate the app's widgets.
// ---------------------------------------------------------------------------

/// Called by `Window::handle_event` in place of ordinary dispatch. Returns
/// `true` when the event was swallowed (the window must NOT hand it to its
/// view children). Off: one atomic load (plus a shortcut check on key events).
/// Commit a pick: climb if this is a repeat click on the same spot, pin what
/// was resolved, and take the caret off the panel.
///
/// Split out of the press handler because in the exploded view the press is
/// not yet a click — it may be the first pixel of an orbit — so the commit
/// waits for a release that never travelled.
fn commit_pick(
    cx: &mut Cx,
    tweaker: &WidgetRef,
    abs: Vec2d,
    window_id: usize,
    pick: Option<TweakPick>,
) {
            let deep_uid = pick.as_ref().map_or(0, |p| p.uid);
            // CLICK-TO-CLIMB: clicking the SAME widget again walks the
            // pin UP one ancestor per click — the only way a container
            // fully covered by its children (the pane that draws the
            // rounded background) can ever be reached. At the top the
            // climb wraps back to the deepest pick. The climb continues
            // only while the presses land on the widget it started from:
            // a press on a different widget inside the pinned container
            // picks that widget. (Every press inside the container used
            // to climb — a click on a sibling button after re-clicking
            // one landed on a bare View, and its draw_bg well was empty.)
            let (pick, climbed) = {
                let (pinned, origin) = {
                    let s = session().lock().unwrap();
                    (s.pinned.clone(), s.climb_origin)
                };
                match (pick, pinned) {
                    (Some(deep), Some(pin))
                        if pin.window_id == window_id
                            && pin.rect.contains(abs)
                            && (deep.uid == pin.uid
                                || (origin == deep.uid
                                    && is_ancestor_of(cx, pin.uid, deep.uid))) =>
                    {
                        (
                            Some(
                                ancestor_pick(cx, &pin, abs, window_id)
                                    .unwrap_or(deep),
                            ),
                            true,
                        )
                    }
                    (deep, _) => (deep, false),
                }
            };
            let mut s = session().lock().unwrap();
            s.climb_origin = deep_uid;
            match &pick {
                Some(pick) => {
                    log!(
                        "TWEAK pick {} ({}) rect {:.0},{:.0} {:.0}x{:.0}{}{}",
                        pick.path,
                        pick.ty,
                        pick.rect.pos.x,
                        pick.rect.pos.y,
                        pick.rect.size.x,
                        pick.rect.size.y,
                        match &pick.band {
                            Some(band) => format!(" band {band}"),
                            None => String::new(),
                        },
                        if climbed { " (climb)" } else { "" }
                    );
                    s.pinned = Some(pick.clone());
                }
                None => {
                    s.pinned = None;
                }
            }
            drop(s);
            // A pick in the body takes the caret off whatever panel
            // field held it. The keys that act on a SELECTION — the
            // hierarchy arrows, Cmd+Z — are only ever ours when nothing
            // is being typed into, and a click on the app is the moment
            // the typing ended.
            cx.set_key_focus(Area::Empty);
            sidebar_refresh(cx, tweaker);
            redraw_tweaker(cx, tweaker);
}

/// Does the design overlay's own chrome sit under `abs`?
///
/// The caption bar spans the whole window and the panel is drawn OVER it, so
/// the window's `WindowDragQuery` would answer "title bar" for the top of the
/// panel — and a press there starts an OS window drag instead of reaching the
/// app. Nothing in that strip can be clicked, which is the filter field, the
/// note button and the extrusion scrub. The window asks this first and
/// answers Client where the overlay is.
pub fn panel_owns_pointer(abs: Vec2d) -> bool {
    if !tweak_is_on() {
        return false;
    }
    let s = session().lock().unwrap();
    s.chrome_band.is_some_and(|r| r.contains(abs))
        || s.chrome_float.iter().any(|r| r.contains(abs))
}

pub fn window_intercept(
    cx: &mut Cx,
    event: &Event,
    window_view: &mut View,
    window_id: WindowId,
) -> bool {
    // Shift+F10 toggles the mode, bridge or no bridge: the design surface is
    // in-process and owes the remote nothing. Only the HTTP endpoints and
    // the AI vibecode loop need --remote; without it they simply are not
    // there, and the panel still is.
    //
    // Ctrl+F10 is not ours: that is the screen recorder
    // (widgets/src/screen_cap.rs), and it must not drag the design surface
    // into every recording.
    if let Event::KeyDown(key_event) = event {
        if key_event.is_tweaker_toggle() {
            let flip = {
                let mut s = session().lock().unwrap();
                if s.toggle_event_id != cx.event_id() {
                    s.toggle_event_id = cx.event_id();
                    true
                } else {
                    false
                }
            };
            if flip {
                set_tweak_on(cx, !tweak_is_on());
            }
            return true;
        }
        return false;
    }
    if !tweak_is_on() {
        return false;
    }

    let (abs, kind) = match event {
        Event::MouseMove(e) if e.window_id == window_id => (e.abs, PointerKind::Move),
        Event::MouseDown(e) if e.window_id == window_id => (e.abs, PointerKind::Down),
        Event::MouseUp(e) if e.window_id == window_id => (e.abs, PointerKind::Up),
        Event::Scroll(e) if e.window_id == window_id => (e.abs, PointerKind::Scroll),
        _ => return false,
    };

    let body = window_view
        .children
        .iter()
        .find(|(id, _)| *id == live_id!(body))
        .map(|(_, widget)| widget.clone());
    let tweaker = window_view
        .children
        .iter()
        .find(|(id, _)| *id == live_id!(tweaker))
        .map(|(_, widget)| widget.clone());
    let (Some(body), Some(tweaker)) = (body, tweaker) else {
        return false;
    };
    let body_rect = body.area().clipped_rect(cx);
    {
        // Hover-reveal for the radius handles: redraw the overlay when the
        // pointer crosses into (or out of) reach of a pinned corner, so the
        // dots appear and vanish without waiting for another repaint cause.
        let mut sess = session().lock().unwrap();
        let was = sess.pointer_abs;
        sess.pointer_abs = abs;
        if let Some(pin) = sess.pinned.clone() {
            let near_of = |p: Vec2d| {
                Tweaker::radius_handle_centers(pin.rect).iter().any(|c| {
                    let dx = p.x - c.x;
                    let dy = p.y - c.y;
                    dx * dx + dy * dy <= 28.0 * 28.0
                })
            };
            if near_of(was) != near_of(abs) {
                drop(sess);
                if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                    tw.redraw_overlay(cx);
                }
            }
        }
    }

    // Scroll routes by REGION, exclusively. Over the panel band: the
    // tweaker tree alone gets it (the body extends under the panel and
    // double-scrolled otherwise). Over the body: ordinary dispatch — the
    // app scrolls, and the overlay re-reads live rects each frame so the
    // outlines follow the content.
    if kind == PointerKind::Scroll {
        // ISOLATED: the wheel magnifies instead of scrolling. There is one
        // thing on screen and the rest is covered, so scrolling the app under
        // it is not what the wheel is for any more.
        let isolated = tweaker
            .borrow::<Tweaker>()
            .is_some_and(|tw| tw.tree_isolate && tw.isolate_uid != 0);
        if isolated {
            if let Event::Scroll(e) = event {
                let step = if e.scroll.y > 0.0 { -ZOOM_WHEEL_STEP } else { ZOOM_WHEEL_STEP };
                if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                    tw.view_zoom = (tw.view_zoom.max(1.0) + step).clamp(1.0, 4.0);
                }
                let zoom = tweaker.borrow::<Tweaker>().map(|tw| tw.view_zoom).unwrap_or(1.0);
                let _ = zoom;
                if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                    tw.view_focus_pending = true;
                }
                redraw_tweaker(cx, &tweaker);
                return true;
            }
        }
        let in_band = tweaker
            .borrow::<Tweaker>()
            .map(|tw| tw.band.size.x > 0.0 && tw.band.contains(abs))
            .unwrap_or(false);
        if in_band {
            tweaker.handle_event(cx, event, &mut Scope::empty());
            return true;
        }
        // EXPLODED, nothing isolated: the wheel opens and closes the stack.
        // The extrusion is the one thing the eye is adjusting in that mode,
        // and it is the only control for it that does not mean crossing the
        // window to the panel's scrub field.
        if cx.sploded_active() {
            if let Event::Scroll(e) = event {
                let step = if e.scroll.y > 0.0 {
                    -SPREAD_WHEEL_STEP
                } else {
                    SPREAD_WHEEL_STEP
                };
                let spread = cx.sploded_spread() + step;
                cx.sploded_set_spread(spread);
                redraw_tweaker(cx, &tweaker);
                return true;
            }
        }
        return false;
    }

    // A pin badge is a mark on the canvas that opens its note. It is checked
    // before anything else picks, because it sits ON the widget it belongs to
    // and a click there means the note, not the widget. With the selection
    // locked it stops answering: opening a badge re-selects its widget, which
    // is the one thing the lock forbids, and a mark that ate a button press
    // would undo the point of handing the mouse back.
    if kind == PointerKind::Down && !session().lock().unwrap().selection_locked {
        let hit = tweaker.borrow::<Tweaker>().and_then(|tw| {
            tw.badge_rects
                .iter()
                .find(|(rect, _)| rect.contains(abs))
                .map(|(_, uid)| *uid)
        });
        if let Some(uid) = hit {
            if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                tw.badge_open = Some(uid);
            }
            session().lock().unwrap().down_consumed = true;
            redraw_tweaker(cx, &tweaker);
            return true;
        }
    }

    // The note card lives on the CANVAS but belongs to the tweaker: input
    // inside it goes to ordinary dispatch (never picked through). The card
    // is OPAQUE to picking — nothing behind it can be hovered or selected,
    // however much of the app it covers — and while it is being dragged or
    // resized the gesture owns the pointer wherever it wanders.
    {
        // A SCRUB leaves the control it started on within a few pixels, and
        // from then on the moves have to keep reaching it — otherwise the
        // value follows the pointer for three pixels and then stops dead,
        // which is the extrusion field becoming un-draggable the moment it
        // left the panel. The press claims the pointer; the release frees it.
        let spread_drag = tweaker.borrow::<Tweaker>().map(|tw| tw.spread_drag).unwrap_or(false);
        // The extrusion readout is the same kind of thing: the tweaker's own
        // chrome sitting on the canvas, and a press in it is a scrub, never
        // a pick.
        let spread_rect = tweaker.borrow::<Tweaker>().and_then(|tw| tw.spread_rect);
        let hits = |rect: Option<Rect>| {
            rect.map(|rect| match event {
                Event::MouseDown(e) => rect.contains(e.abs),
                Event::MouseMove(e) => rect.contains(e.abs),
                Event::MouseUp(e) => rect.contains(e.abs),
                _ => false,
            })
            .unwrap_or(false)
        };
        let on_spread = hits(spread_rect);
        if kind == PointerKind::Down && on_spread {
            if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                tw.spread_drag = true;
            }
        }
        if kind == PointerKind::Up && spread_drag {
            if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                tw.spread_drag = false;
            }
        }
        if on_spread || spread_drag {
            if kind == PointerKind::Down {
                log!("TWEAK press {:.0},{:.0} on the tweaker's chrome: not a pick", abs.x, abs.y);
            }
            if kind == PointerKind::Move {
                // Whatever was outlined under the card stops being: the
                // pointer is on the note, not on the app.
                let had_hover = session().lock().unwrap().hover.take().is_some();
                if had_hover {
                    redraw_tweaker(cx, &tweaker);
                }
            }
            return false;
        }
    }
    // A scrub pin owns the pointer ABSOLUTELY: no picking, no hover-outline
    // churn at the virtual abs (the "keeps highlighting things while I
    // drag" bug), and never a consumed up (the wedge). The pick pass
    // stands down until the pin releases; stale hover chrome clears once.
    // A NEW press while a pin stands means the pin's release was lost (the
    // button cannot be held twice): drop it and pick, instead of feeding
    // one whole click to a gesture that ended.
    if cx.fingers.has_pinned_capture() {
        if kind == PointerKind::Down {
            log!("TWEAK press {:.0},{:.0} with a stale scrub pin: released, picking", abs.x, abs.y);
            cx.unpin_pointer_capture();
        } else {
            let had_hover = {
                let mut s = session().lock().unwrap();
                s.hover.take().is_some()
            };
            if had_hover {
                cx.redraw_all();
            }
            return false;
        }
    }
    // A splitter drag owns the pointer outright: the pick path must not
    // eat the Move that resizes or the Up that releases — releasing
    // OUTSIDE the band swallowed the Up and wedged the drag forever. The
    // same stale-gesture rule as the pin: a new press ends it.
    {
        let dragging = tweaker
            .borrow::<Tweaker>()
            .map(|tw| tw.splitter_drag)
            .unwrap_or(false);
        if dragging {
            if kind == PointerKind::Down {
                log!("TWEAK press {:.0},{:.0} with a stale splitter drag: ended, picking", abs.x, abs.y);
                if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                    tw.splitter_drag = false;
                }
            } else {
                if kind == PointerKind::Move {
                    // Keep the resize cursor for the whole drag, wherever
                    // the pointer wanders.
                    cx.set_cursor(MouseCursor::EwResize);
                }
                return false;
            }
        }
    }
    // A mouse-up always pairs with the down that started it: if we swallowed
    // the down, swallow the up wherever it lands.
    let finish_consumed_down = kind == PointerKind::Up && session().lock().unwrap().down_consumed;
    if !body_rect.contains(abs) && !finish_consumed_down {
        // Outside the body (caption bar, sidebar band): ordinary dispatch.
        if kind == PointerKind::Down {
            log!(
                "TWEAK press {:.0},{:.0} outside the body {:.0},{:.0} {:.0}x{:.0}: not a pick",
                abs.x, abs.y, body_rect.pos.x, body_rect.pos.y, body_rect.size.x, body_rect.size.y
            );
        }
        return false;
    }
    // The tweaker's own UI is never a pick target. A body that was not
    // vacated (apps whose layout ignores the sidebar apply) still overlaps
    // the band, so check it explicitly: a pointer over the panel goes to
    // ordinary dispatch — the panel wins INPUT, and the app widget behind
    // it can never be selected through it.
    if !finish_consumed_down {
        let band = tweaker.borrow::<Tweaker>().map(|tw| tw.band).unwrap_or_default();
        // The panel splitter announces itself: the resize cursor over its
        // grab band (it is the intercept's own gesture, not a Splitter
        // widget, so nothing else would set one).
        if kind == PointerKind::Move
            && band.size.x > 0.0
            && abs.x >= band.pos.x - 3.0
            && abs.x <= band.pos.x + SPLITTER_WIDTH + 3.0
            && abs.y >= band.pos.y
        {
            cx.set_cursor(MouseCursor::EwResize);
            return false;
        }
        let in_band = band.size.x > 0.0 && band.contains(abs);
        if in_band {
            if kind == PointerKind::Down {
                log!("TWEAK press {:.0},{:.0} in the panel band: not a pick", abs.x, abs.y);
            }
            if kind == PointerKind::Move {
                // The body's pick-hand must not linger over the panel; the
                // panel's own widgets set theirs (I-beam etc.) after this.
                cx.set_cursor(MouseCursor::Default);
            }
            return false;
        }
    }

    // Eyedropper armed: the press samples the pixel under it from the next
    // presented frame (device pixels), the tweaker's event loop applies it.
    if kind == PointerKind::Down {
        let armed = session().lock().unwrap().eyedrop.take();
        if let Some(prop) = armed {
            let dpi = cx.windows[window_id].window_geom.dpi_factor;
            let id = cx.probe_pixel((abs.x * dpi) as u32, (abs.y * dpi) as u32);
            session().lock().unwrap().eyedrop_probe = Some((id, prop));
            session().lock().unwrap().down_consumed = true;
            return true;
        }
    }
    // Annotate mode draws; holding Alt draws too, so a person can sketch a
    // note mid-pick without flipping the mode.
    let alt_held = match event {
        Event::MouseMove(e) => e.modifiers.alt,
        Event::MouseDown(e) => e.modifiers.alt,
        Event::MouseUp(e) => e.modifiers.alt,
        _ => false,
    };
    let annotate = session().lock().unwrap().annotate || alt_held;

    // SELECT OFF: the overlay hands the mouse back to the app.
    //
    // Everything above this line is the overlay's OWN surfaces — the panel
    // band, the splitter, the note card, its popovers — and they keep working
    // because they are not the app. Everything below is picking: hover
    // outlines, the click that selects, the direct-manipulation handles. All
    // of it stands down, so a button under the pointer is just a button, and
    // the selection stays exactly where it was for the panel to keep editing.
    // Annotate is its own mode and is exempt: sketching over a live app is
    // precisely what it is for.
    if !annotate && session().lock().unwrap().selection_locked {
        if kind == PointerKind::Move {
            let stale = session().lock().unwrap().hover.take().is_some();
            if stale {
                redraw_tweaker(cx, &tweaker);
            }
        }
        return false;
    }

    // Direct manipulation first: the corner handles of a radius-carrying
    // selection own their presses before picking does — unless the view is
    // centred or zoomed, in which case the handles are not drawn and must
    // not swallow presses at the layout corners they no longer sit on.
    if !annotate && !cx.sploded_transformed() {
        match kind {
            PointerKind::Down => {
                let pinned = session().lock().unwrap().pinned.clone();
                if let Some(pin) = pinned.filter(|p| p.window_id == window_id.id()) {
                    let hit = {
                        let tw = tweaker.borrow::<Tweaker>();
                        tw.and_then(|tw| {
                            tw.radius_prop.clone().and_then(|(prop, value)| {
                                Tweaker::radius_handle_centers(pin.rect)
                                    .iter()
                                    .position(|center| {
                                        let dx = abs.x - center.x;
                                        let dy = abs.y - center.y;
                                        dx * dx + dy * dy <= 49.0
                                    })
                                    .map(|corner| (corner, prop, value))
                            })
                        })
                    };
                    if let Some((corner, _prop, value)) = hit {
                        if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                            tw.radius_drag = Some((corner, value, abs));
                        }
                        {
                            let mut s = session().lock().unwrap();
                            s.down_consumed = true;
                            s.edit_hold = true;
                        }
                        redraw_tweaker(cx, &tweaker);
                        return true;
                    }
                }
            }
            PointerKind::Move => {
                let drag = tweaker
                    .borrow::<Tweaker>()
                    .and_then(|tw| tw.radius_drag.map(|d| (d, tw.radius_prop.clone())));
                if let Some(((corner, start_value, start_pos), Some((prop, _)))) = drag {
                    let inward = Tweaker::radius_inward(corner);
                    let delta = dvec2(abs.x - start_pos.x, abs.y - start_pos.y);
                    let travel = (delta.x * inward.x + delta.y * inward.y) * 0.5;
                    let value = (start_value + travel).max(0.0);
                    let sel = session().lock().unwrap().pinned.clone();
                    if let Some(sel) = sel {
                        let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
                        if !widget.is_empty() {
                            let chunk = format!("{}: {}", prop, fmt_f64(value));
                            if let Err(error) =
                                apply_splash_chunk(cx, &widget, &sel.path, &chunk, "handle")
                            {
                                log!("TWEAK handle apply failed: {error}");
                            }
                        }
                    }
                    cx.set_cursor(MouseCursor::Crosshair);
                    redraw_tweaker(cx, &tweaker);
                    return true;
                }
            }
            PointerKind::Up => {
                let was_dragging = tweaker
                    .borrow::<Tweaker>()
                    .is_some_and(|tw| tw.radius_drag.is_some());
                if was_dragging {
                    if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                        tw.radius_drag = None;
                        tw.rows_uid = 0;
                    }
                    {
                        let mut s = session().lock().unwrap();
                        s.down_consumed = false;
                        s.edit_hold = false;
                    }
                    redraw_tweaker(cx, &tweaker);
                    return true;
                }
            }
            PointerKind::Scroll => {}
        }
    }

    match kind {
        PointerKind::Move => {
            // The colour popover overhangs the body: a move inside it is
            // the panel's (its palette strip hovers), not a hover-pick.
            let popup = session().lock().unwrap().popup;
            if popup.is_some_and(|rect| rect.contains(abs)) {
                tweaker.handle_event(cx, event, &mut Scope::empty());
                return true;
            }
            // The doc tooltip closes when the pointer leaves into the body
            // (the panel never sees these moves).
            if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                tw.doc_tip_hover(cx, abs);
                tw.scope_tip_hover(cx, abs);
                tw.chrome_tip_hover(cx, abs);
                tw.states_hover(cx, abs);
                tw.pulse_hover(cx, abs);
            }
            let eyedrop = session().lock().unwrap().eyedrop.is_some();
            cx.set_cursor(if annotate || eyedrop {
                MouseCursor::Crosshair
            } else {
                MouseCursor::Hand
            });
            let drawing = session().lock().unwrap().live_stroke.is_some();
            if drawing {
                let mut s = session().lock().unwrap();
                if let Some(stroke) = &mut s.live_stroke {
                    stroke.points.push((abs.x, abs.y));
                }
                drop(s);
                redraw_tweaker(cx, &tweaker);
            } else {
                let pick = resolve_pick(cx, &body, abs, window_id.id());
                let mut s = session().lock().unwrap();
                if s.hover != pick {
                    s.hover = pick;
                    drop(s);
                    redraw_tweaker(cx, &tweaker);
                }
            }
        }
        PointerKind::Down => {
            // MIDDLE CLICK: in and out of the exploded view. The mode is a
            // way of LOOKING at the app, and reaching for a key or crossing
            // to the panel to get into it interrupts the looking. Only over
            // the body — a middle click in the panel band never reaches here
            // — and only while the overlay is up, so an ordinary app keeps
            // whatever it does with the wheel button.
            if let Event::MouseDown(e) = event {
                if e.button.is_middle() {
                    cx.sploded_toggle();
                    session().lock().unwrap().down_consumed = true;
                    redraw_tweaker(cx, &tweaker);
                    return true;
                }
                // RIGHT CLICK: say something about this one. It selects the
                // widget and opens its note in a single gesture, which is
                // the whole point — writing a note used to mean a click to
                // select and then a key to open. Never a toggle: a right
                // click means "the note for THAT", so a card already open on
                // another widget moves rather than closing.
                if e.button.is_secondary() {
                    let pick = resolve_pick(cx, &body, abs, window_id.id());
                    if let Some(pick) = pick {
                        if let Some(mut tw) = tweaker.borrow_mut::<Tweaker>() {
                            tw.badge_open = Some(pick.uid);
                        }
                    }
                    session().lock().unwrap().down_consumed = true;
                    redraw_tweaker(cx, &tweaker);
                    return true;
                }
            }
            {
                session().lock().unwrap().down_consumed = true;
            }
            // A sidebar interaction (color popover, focused field) owns this
            // press: hand it to the tweaker so the popover closes / the
            // field commits — the selection does NOT change.
            if session().lock().unwrap().edit_hold {
                tweaker.handle_event(cx, event, &mut Scope::empty());
                // The press ends the interaction whatever the widgets made
                // of it (a popover that closed already said so; a focused
                // field may not report focus loss for an off-widget press).
                // The matching release goes the same way, so a button
                // inside the popover (the eyedropper's `pick`) can finish
                // its click.
                let mut s = session().lock().unwrap();
                s.edit_hold = false;
                s.hold_up = true;
                drop(s);
                redraw_tweaker(cx, &tweaker);
                // A press on the tweaker's own chrome over the body (the
                // colour popover) stops here. A press on the APP both ends
                // the field/popover and picks what it landed on — one
                // click, like a click with nothing focused; the first click
                // after typing in a field used to be swallowed.
                let attached = attached_lists_of(cx, &tweaker);
                if chrome_hit(cx, &tweaker, abs, &attached, 0) {
                    return true;
                }
            }
            if annotate {
                let mut stroke = TweakStroke::default();
                stroke.window_id = window_id.id();
                stroke.points.push((abs.x, abs.y));
                if let Some(pick) = resolve_pick(cx, &body, abs, window_id.id()) {
                    stroke.widgets.push(pick.path);
                }
                session().lock().unwrap().live_stroke = Some(stroke);
            } else {
                let pick = resolve_pick(cx, &body, abs, window_id.id());
                // @MENTION: an `@` in the open note armed a reference. This
                // click names a widget INTO the note text and leaves the
                // selection alone — the note still belongs to the widget it
                // was opened on; the mention is what it points AT.
                if session().lock().unwrap().mention {
                    session().lock().unwrap().mention = false;
                    // The field a mention lands in is the one the @ was
                    // typed into, remembered at arming: the prompt strip is
                    // on every tab, so which tab is up says nothing. A pick
                    // made for a mention never moves the selection (this arm
                    // returns before `commit_pick`), so the tab stays on the
                    // widget the note is about.
                    let into_notes = !session().lock().unwrap().mention_from_prompt;
                    let field = tweaker.borrow::<Tweaker>().and_then(|tw| {
                        tw.sidebar.as_ref().map(|sidebar| {
                            if into_notes {
                                sidebar
                                    .child(live_id!(spec_col))
                                    .child(live_id!(notes_box))
                                    .child(live_id!(spec_notes))
                            } else {
                                sidebar.child(live_id!(prompt_row)).child(live_id!(prompt_field))
                            }
                        })
                    });
                    match (field, &pick) {
                        (Some(field), Some(pick)) => {
                            // The reference is the mentioned widget's INDEXED
                            // path — the only form that names one widget —
                            // written relative to the note's own widget when
                            // the absolute form is too long for the card. Both
                            // resolve to the same thing; the short one is just
                            // readable.
                            // The base is the note the field is SHOWING, not
                            // whatever happens to be pinned: those can differ
                            // for a frame, and writing one note's text into
                            // another's is how a note gets lost.
                            let base = tweaker
                                .borrow::<Tweaker>()
                                .map(|tw| tw.note_key_shown.clone())
                                .unwrap_or_default();
                            if base.is_empty() {
                                return true;
                            }
                            // Two ways to say it: the widget's own reference,
                            // and its position relative to the noted widget.
                            // Take whichever is shorter — the relative form
                            // usually wins for anything nearby, and it says
                            // MORE while it does: `./Label_2` is "the one
                            // next to this".
                            let target = indexed_path(cx, pick.uid);
                            let reference = relative_path(&base, &target)
                                .into_iter()
                                .chain(std::iter::once(target.clone()))
                                .min_by_key(|form| form.len())
                                .unwrap_or_else(|| target.clone());
                            let text = insert_mention(&field.text(), &reference);
                            field.set_text(cx, &text);
                            log!("TWEAK @mention {reference} -> {target} ({})", pick.ty);
                            // Into whichever side is on screen: a mention
                            // typed into the prompt is part of the
                            // instruction, not of the note.
                            if into_notes {
                                let notes = {
                                    let mut s = session().lock().unwrap();
                                    if let Some(note) =
                                        s.notes.iter_mut().find(|n| n.path == base)
                                    {
                                        note.text = text;
                                    }
                                    s.notes.clone()
                                };
                                note_store_save(&notes);
                            } else {
                                session().lock().unwrap().prompt = text;
                            }
                        }
                        _ => log!("TWEAK @mention: nothing under the click"),
                    }
                    session().lock().unwrap().down_consumed = true;
                    redraw_tweaker(cx, &tweaker);
                    return true;
                }
                // In the exploded view a press is not yet a click: it may
                // be the first pixel of an ORBIT, and an orbit is a change of
                // viewpoint, not of selection. Hold the pick for the release
                // and take it only if the pointer stayed where it was put.
                if cx.sploded_active() {
                    session().lock().unwrap().press_pick = Some(abs);
                } else {
                    commit_pick(cx, &tweaker, abs, window_id.id(), pick.clone());
                }
                // Navigation stays reachable: the press flows on to the
                // tab / fold / dropdown as well, and so will its release.
                if let Some(pick) = &pick {
                    if is_navigation_pick(cx, WidgetUid(pick.uid)) {
                        let mut s = session().lock().unwrap();
                        s.down_consumed = false;
                        s.pass_up = true;
                        return false;
                    }
                }
            }
        }
        PointerKind::Up => {
            let mut s = session().lock().unwrap();
            if s.pass_up {
                s.pass_up = false;
                s.down_consumed = false;
                return false;
            }
            if s.hold_up {
                s.hold_up = false;
                s.down_consumed = false;
                drop(s);
                tweaker.handle_event(cx, event, &mut Scope::empty());
                redraw_tweaker(cx, &tweaker);
                return true;
            }
            s.down_consumed = false;
            // The held press: a release that has not travelled is a click,
            // and only now is it safe to say so.
            if let Some(down) = s.press_pick.take() {
                let still = (down.x - abs.x).abs() <= CLIMB_SLOP
                    && (down.y - abs.y).abs() <= CLIMB_SLOP;
                if still {
                    drop(s);
                    let pick = resolve_pick(cx, &body, down, window_id.id());
                    commit_pick(cx, &tweaker, down, window_id.id(), pick);
                    return true;
                }
                // Travelled: it was an orbit. The guard is still held and
                // must stay that way — re-locking it here deadlocks the
                // session mutex against itself, which hangs the app.
            }
            if let Some(mut stroke) = s.live_stroke.take() {
                stroke.points.push((abs.x, abs.y));
                // Tag the widgets the stroke touches: resolve its endpoints
                // and midpoint.
                s.strokes.push(stroke);
                s.drew = true;
                let stroke_index = s.strokes.len() - 1;
                drop(s);
                tag_stroke_widgets(cx, &body, window_id.id(), stroke_index);
                redraw_tweaker(cx, &tweaker);
            }
        }
        PointerKind::Scroll => {
            // Swallowed: scrolling would move the thing being pointed at.
        }
    }
    true
}

#[derive(Clone, Copy, PartialEq)]
enum PointerKind {
    Move,
    Down,
    Up,
    Scroll,
}

/// Does a point land on the tweaker's own widgets (below its root overlay,
/// which spans the window)?
fn chrome_hit(
    cx: &Cx,
    widget: &WidgetRef,
    abs: Vec2d,
    attached: &HashSet<DrawListId>,
    depth: usize,
) -> bool {
    if depth > 0 && pick_candidate(cx, widget, abs, attached).is_some() {
        return true;
    }
    if depth > 24 {
        return false;
    }
    let mut kids: Vec<WidgetRef> = Vec::new();
    widget.children(&mut |_id, child| kids.push(child));
    kids
        .iter()
        .any(|child| chrome_hit(cx, child, abs, attached, depth + 1))
}

fn redraw_tweaker(cx: &mut Cx, tweaker: &WidgetRef) {
    if let Some(mut tweaker) = tweaker.borrow_mut::<Tweaker>() {
        tweaker.redraw_overlay(cx);
    }
}

fn sidebar_refresh(cx: &mut Cx, tweaker: &WidgetRef) {
    if let Some(mut tweaker) = tweaker.borrow_mut::<Tweaker>() {
        // Force the rows to rebuild on the next draw, even for a re-pin of
        // the same widget (its values may have moved underneath).
        tweaker.rows_uid = 0;
        let _ = cx;
    }
}

fn tag_stroke_widgets(cx: &mut Cx, body: &WidgetRef, window_id: usize, stroke_index: usize) {
    let points: Vec<(f64, f64)> = {
        let s = session().lock().unwrap();
        match s.strokes.get(stroke_index) {
            Some(stroke) => stroke.points.clone(),
            None => return,
        }
    };
    let mut widgets: Vec<String> = Vec::new();
    // Sample the stroke sparsely; resolving every point would walk the tree
    // hundreds of times for one squiggle.
    let step = (points.len() / 8).max(1);
    for (index, (x, y)) in points.iter().enumerate() {
        if index % step != 0 && index != points.len() - 1 {
            continue;
        }
        if let Some(pick) = resolve_pick(cx, body, dvec2(*x, *y), window_id) {
            if !widgets.contains(&pick.path) {
                widgets.push(pick.path);
            }
        }
    }
    let mut s = session().lock().unwrap();
    if let Some(stroke) = s.strokes.get_mut(stroke_index) {
        stroke.widgets = widgets;
    }
}

// ---------------------------------------------------------------------------
// reflection — walk the selected widget's applied script object (own map =
// explicitly set, proto chain = the type's live/shader-input registry) into
// a property list with real current values.
// ---------------------------------------------------------------------------

/// Keys that are structure, not style: reflected objects skip them.
fn skip_key(name: &str) -> bool {
    matches!(name, "animator")
}

/// Draw-shader plumbing the GPU pipeline owns — never user style. Skipped in
/// the dotted expansion so the sidebar shows design inputs, not internals.
fn is_shader_plumbing(name: &str) -> bool {
    matches!(
        name,
        "rect_pos"
            | "rect_size"
            | "draw_clip"
            | "delta_clip"
            | "draw_depth"
            | "draw_zbias"
            | "char_index"
            | "texture_index"
            | "atlas_plane"
            | "t"
            | "t_min"
            | "t_max"
            | "temp_y_shift"
            | "aa_2x2"
            | "aa_4x4"
            | "pad1"
            | "pad2"
            | "font_scale"
            | "depth_clip"
            | "debug"
            | "stem_darken_max"
            | "draw_scroll"
            | "view_shift"
            | "view_clip"
            | "camera_projection"
            | "camera_view"
            | "camera_inv"
            | "dpi_factor"
            | "dpi_dilate"
            | "seed"
            | "pos"
            | "vertex_pos"
            | "total_chars"
            | "char_depth"
            | "draw_call"
            | "draw_list"
            | "draw_pass"
            | "geom"
            | "world"
            | "extend_area"
            | "ink_centered"
            | "sdf_sharpness"
            | "sdf_luma_bias"
            | "draw_flags"
            | "clip_x"
            | "clip_y"
    )
}

fn fmt_scalar(heap: &ScriptHeap, value: ScriptValue) -> Option<String> {
    if value.is_nil() {
        return Some("null".to_string());
    }
    if let Some(color) = value.as_color() {
        return Some(format!("#{color:08x}"));
    }
    if let Some(b) = value.as_bool() {
        return Some(if b { "true" } else { "false" }.to_string());
    }
    if let Some(n) = value.as_number() {
        if n.fract() == 0.0 && n.abs() < 1.0e15 {
            return Some(format!("{}", n as i64));
        }
        return Some(format!("{}", (n * 10000.0).round() / 10000.0));
    }
    if let Some(id) = value.as_id() {
        return Some(format!("@{}", live_id_token(id)));
    }
    if value.is_string_like() {
        if let Some(text) = heap.string_with(value, |_, s| format!("{s:?}")) {
            return Some(text);
        }
        return Some("\"\"".to_string());
    }
    if let Some(pod) = value.as_pod() {
        let (pod_type, data) = heap.pod_data(pod);
        let name = pod_type
            .name
            .map(live_id_token)
            .unwrap_or_else(|| "pod".to_string());
        // A unit-range vec4f in UI code is a color; render it the way the
        // splash source would spell it, so diffs paste straight back.
        if name == "vec4f" && data.len() == 4 {
            let mut components = [0.0f32; 4];
            let mut unit = true;
            for (index, raw) in data.iter().enumerate() {
                let v = f32::from_bits(*raw);
                if !(0.0..=1.0).contains(&v) {
                    unit = false;
                    break;
                }
                components[index] = v;
            }
            if unit {
                let to_byte = |v: f32| (v * 255.0).round() as u32;
                return Some(format!(
                    "#{:02x}{:02x}{:02x}{:02x}",
                    to_byte(components[0]),
                    to_byte(components[1]),
                    to_byte(components[2]),
                    to_byte(components[3])
                ));
            }
        }
        let mut out = format!("{name}(");
        for (index, raw) in data.iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            let v = f32::from_bits(*raw);
            if v.fract() == 0.0 {
                out.push_str(&format!("{}", v as i64));
            } else {
                out.push_str(&format!("{v}"));
            }
        }
        out.push(')');
        return Some(out);
    }
    None
}

/// Splash-ish one-line rendering of a value, `depth` levels of objects deep.
/// How an enum value is written out. `Full` names the enum and carries
/// every field that is set, so the text re-applies exactly under
/// `Apply::Eval`; `Display` is for a row's eye and drops the enum's name and
/// the fields equal to the variant's own defaults.
#[derive(Clone, Copy, PartialEq)]
enum EnumFmt {
    Full,
    Display,
}

/// (enum, variant) for an object that came from a derived enum, else None.
/// The derive stamps `__enum` on every variant object -- a bare variant, a
/// tuple, a named one -- and instances inherit it through the proto chain;
/// the variant itself is the first bare id in that chain.
fn enum_info(heap: &ScriptHeap, obj: ScriptObject) -> Option<(LiveId, LiveId)> {
    let en = heap.value(obj, live_id!(__enum).into(), NoTrap).as_id()?;
    let mut ptr = obj;
    for _ in 0..8 {
        let proto = heap.proto(ptr);
        if let Some(id) = proto.as_id() {
            return Some((en, id));
        }
        ptr = proto.as_object()?;
    }
    None
}

/// The nearest proto that is an object: for an instance of a named variant
/// that is the frozen variant itself, which holds the field defaults.
fn enum_variant_proto(heap: &ScriptHeap, obj: ScriptObject) -> Option<ScriptObject> {
    heap.proto(obj).as_object()
}

/// `Flow.Down`, `Size.Fixed(200)`, `Size.Fill{weight: 100 basis: 0 shrink: 0}`.
/// A relative size prints as the CSS it was written as: `50%`, `25vw`.
fn fmt_enum(heap: &ScriptHeap, obj: ScriptObject, fmt: EnumFmt) -> Option<String> {
    let (en, var) = enum_info(heap, obj)?;
    let en_name = live_id_token(en);
    let var_name = live_id_token(var);

    // A relative size is a string in the person's head, not a struct.
    if var_name == "Rel" && (en_name == "Size" || en_name == "FitBound") {
        let factor = heap.value(obj, live_id!(factor).into(), NoTrap).as_number();
        let base = heap
            .value(obj, live_id!(base).into(), NoTrap)
            .as_object()
            .and_then(|b| enum_info(heap, b))
            .map(|(_, v)| live_id_token(v));
        if let (Some(factor), Some(base)) = (factor, base) {
            let unit = match base.as_str() {
                "Parent" => Some("%"),
                "Vw" => Some("vw"),
                "Vh" => Some("vh"),
                "Cqw" => Some("cqw"),
                "Cqh" => Some("cqh"),
                _ => None,
            };
            if let Some(unit) = unit {
                return Some(format!("{}{unit}", fmt_f64(factor * 100.0)));
            }
        }
    }

    let head = match fmt {
        EnumFmt::Full => format!("{en_name}.{var_name}"),
        EnumFmt::Display => var_name.clone(),
    };

    // Tuple: the positional values.
    let len = heap.vec_len(obj);
    if len > 0 {
        let mut out = head;
        out.push('(');
        for index in 0..len {
            if index > 0 {
                out.push_str(", ");
            }
            let value = heap.vec_value_if_exist(obj, index)?;
            out.push_str(&fmt_value(heap, value, 2).unwrap_or_else(|| "?".to_string()));
        }
        out.push(')');
        return Some(out);
    }

    // Named: the instance's own fields first, in the order they were set,
    // then the variant's defaults it did not set. Hidden keys stay hidden.
    let mut keys: Vec<LiveId> = Vec::new();
    collect_prop_keys(heap, obj, &mut keys);
    keys.retain(|k| *k != live_id!(__enum) && !live_id_token(*k).starts_with('_'));
    if keys.is_empty() {
        return Some(head); // bare
    }
    let defaults = enum_variant_proto(heap, obj);
    let mut out = head.clone();
    out.push('{');
    let mut first = true;
    for key in keys {
        let value = heap.value(obj, key.into(), NoTrap);
        if value.is_nil() {
            continue; // an Option that is None
        }
        let Some(text) = fmt_value(heap, value, 2) else { continue };
        if fmt == EnumFmt::Display {
            if let Some(defaults) = defaults {
                let default = heap.value(defaults, key.into(), NoTrap);
                if fmt_value(heap, default, 2).as_deref() == Some(text.as_str()) {
                    continue;
                }
            }
        }
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(&live_id_token(key));
        out.push_str(": ");
        out.push_str(&text);
    }
    if first {
        // Nothing to say beyond the variant: every field is its default
        // (Display) or none is set. `Fill`, not `Fill{}`.
        return Some(head);
    }
    out.push('}');
    Some(out)
}

/// One axis of a size, as the Props tab reads it off a reflected row. The
/// row's text is whatever `fmt_enum` wrote -- `Size.Fill{weight: 100 ..}`,
/// `Size.Fixed(200)`, `Fit`, a bare number from an older row, `"50%"` or
/// `"calc(100% - 20px)"` for the CSS spellings -- and every one of them
/// has to come back as the same handful of shapes the editor draws.
#[derive(Clone, Debug, PartialEq)]
enum SizeText {
    Fill(FillText),
    Fit,
    Fixed(f64),
    /// `50%`, `25vw`, `60cqw`: a size relative to something, as written.
    Rel(String),
    /// `calc(..)`, `min(..)`, `clamp(..)`: a size the layout pass works out.
    Expr(String),
}

/// What a `Fill` carries: how much of the free space it takes against its
/// siblings, where it starts from, how it gives way, and the margin-box
/// bounds a Fill has always had. Read off the printed row and written back
/// whole, since a field of a variant is not a property of its own.
#[derive(Clone, Debug, PartialEq)]
struct FillText {
    weight: f64,
    /// `0`, `25%`, `calc(100% - 20px)`: the basis as written.
    basis: String,
    shrink: f64,
    min: Option<f64>,
    max: Option<f64>,
}

impl FillText {
    /// A Fill with nothing said about it.
    fn plain() -> Self {
        FillText { weight: 100.0, basis: "0".to_string(), shrink: 0.0, min: None, max: None }
    }

    /// The chunk that sets the axis to this Fill.
    fn chunk(&self, axis: &str) -> String {
        let mut out = format!(
            "{axis}: Size.Fill{{weight: {} basis: {} shrink: {}",
            fmt_f64(self.weight),
            size_literal(&self.basis),
            fmt_f64(self.shrink)
        );
        if let Some(min) = self.min {
            out.push_str(&format!(" min: {}", fmt_f64(min)));
        }
        if let Some(max) = self.max {
            out.push_str(&format!(" max: {}", fmt_f64(max)));
        }
        out.push('}');
        out
    }
}

/// A size as the engine reads it: a number stays bare, anything else --
/// `25%`, `calc(100% - 20px)` -- is a string for its parser.
fn size_literal(text: &str) -> String {
    let text = text.trim().trim_matches('"').trim();
    if text.parse::<f64>().is_ok() {
        text.to_string()
    } else {
        format!("\"{text}\"")
    }
}

/// The fields of a printed `Size.Fill{weight: 100 basis: FitBound.Abs(0)
/// shrink: 0}`; anything missing is the engine's own default.
fn parse_fill_text(text: &str) -> FillText {
    FillText {
        weight: field_number(text, "weight").unwrap_or(100.0),
        basis: fill_field(text, "basis").map(bound_text).unwrap_or_else(|| "0".to_string()),
        shrink: field_number(text, "shrink").unwrap_or(0.0),
        min: field_number(text, "min"),
        max: field_number(text, "max"),
    }
}

/// One field of a printed Fill, up to the next field or the closing brace;
/// a quoted value -- an expression with spaces in it -- is taken whole.
fn fill_field<'a>(text: &'a str, field: &str) -> Option<&'a str> {
    let key = format!("{field}: ");
    let start = text.find(&key)? + key.len();
    let rest = &text[start..];
    if let Some(quoted) = rest.strip_prefix('"') {
        let end = quoted.find('"')?;
        return Some(&quoted[..end]);
    }
    let end = [" weight:", " basis:", " shrink:", " min:", " max:", "}"]
        .iter()
        .filter_map(|stop| rest.find(stop))
        .min()
        .unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// A bound as printed -- `FitBound.Abs(120)`, `50%`, `"calc(100% - 20px)"`
/// -- as the text of its field: `120`, `50%`, `calc(100% - 20px)`.
fn bound_text(printed: &str) -> String {
    let text = printed.trim().trim_matches('"').trim();
    let head = text.split(['{', '(']).next().unwrap_or("").trim();
    if head.rsplit('.').next() == Some("Abs") {
        let inner = text.split('(').nth(1).unwrap_or("").trim_end_matches(')').trim();
        return inner.parse::<f64>().map(fmt_f64).unwrap_or_else(|_| inner.to_string());
    }
    text.to_string()
}

/// What was typed into a min / max field: nothing clears the bound, a
/// number is points, and a spelling the engine parses goes in quotes.
/// Fill and Fit mean nothing for a bound, and half-typed text is not sent.
fn bound_chunk(prop: &str, typed: &str) -> Option<String> {
    let typed = typed.trim().trim_matches('"').trim();
    if typed.is_empty() || typed == "-" || typed == "\u{2013}" || typed.eq_ignore_ascii_case("none") {
        return Some(format!("{prop}: nil"));
    }
    match parse_size_text(typed) {
        SizeText::Fixed(v) => Some(format!("{prop}: {}", fmt_f64(v))),
        SizeText::Rel(text) | SizeText::Expr(text) => Some(format!("{prop}: \"{text}\"")),
        SizeText::Fill(_) | SizeText::Fit => None,
    }
}

/// What was typed into the aspect field: nothing clears it, `16:9` or
/// `16/9` is a ratio, a number is width over height; `3:` on the way to
/// `3:2` is not sent.
fn aspect_chunk(typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() || typed == "-" || typed == "\u{2013}" || typed.eq_ignore_ascii_case("none") {
        return Some("aspect: nil".to_string());
    }
    if let Some((w, h)) = typed.split_once([':', '/']) {
        let (w, h) = (w.trim().parse::<f64>().ok()?, h.trim().parse::<f64>().ok()?);
        return (h != 0.0).then(|| format!("aspect: {}", fmt_f64(w / h)));
    }
    typed.parse::<f64>().ok().map(|v| format!("aspect: {}", fmt_f64(v)))
}

impl SizeText {
    /// The value the field shows for it.
    fn field_text(&self) -> String {
        match self {
            SizeText::Fill(_) => "Fill".to_string(),
            SizeText::Fit => "Fit".to_string(),
            SizeText::Fixed(v) => fmt_f64(*v),
            SizeText::Rel(text) | SizeText::Expr(text) => text.clone(),
        }
    }
}

/// Read a size row's text. Tolerant of every spelling the panel has ever
/// shown for one, because rows come from `fmt_value` today and came from
/// dotted fragments and bare numbers before it.
fn parse_size_text(text: &str) -> SizeText {
    let text = text.trim().trim_matches('"').trim();
    if text.is_empty() {
        return SizeText::Fit;
    }
    if let Ok(v) = text.parse::<f64>() {
        return SizeText::Fixed(v);
    }
    // `Size.Fill{weight: 100 ..}`, `Fill{..}`, `Fill`, and the Fit / Fixed
    // shapes: the variant is the word after the last dot before any brace
    // or paren. Tried FIRST, because a printed Fill carries `Abs(0)` inside
    // it and a paren alone would read as an expression.
    let head = text.split(['{', '(']).next().unwrap_or("").trim();
    let variant = head.rsplit('.').next().unwrap_or("").trim();
    match variant {
        "Fill" => return SizeText::Fill(parse_fill_text(text)),
        "Fit" => return SizeText::Fit,
        "Fixed" => {
            let inner = text.split('(').nth(1).unwrap_or("").trim_end_matches(')').trim();
            return SizeText::Fixed(inner.parse().unwrap_or(0.0));
        }
        _ => {}
    }
    let lower = text.to_ascii_lowercase();
    // A relative size or an expression is kept as written: the field shows
    // it as such, and it is emitted back as the same string.
    if lower.contains('(') {
        return SizeText::Expr(text.to_string());
    }
    if let Some(num) = lower
        .strip_suffix("cqw")
        .or_else(|| lower.strip_suffix("cqh"))
        .or_else(|| lower.strip_suffix("vw"))
        .or_else(|| lower.strip_suffix("vh"))
        .or_else(|| lower.strip_suffix('%'))
        .or_else(|| lower.strip_suffix("px"))
    {
        if num.trim().parse::<f64>().is_ok() {
            return if lower.ends_with("px") {
                SizeText::Fixed(num.trim().parse().unwrap_or(0.0))
            } else {
                SizeText::Rel(text.to_string())
            };
        }
    }
    SizeText::Fit
}

/// `weight: 100` out of a printed named variant, when the field is a plain
/// number.
fn field_number(text: &str, field: &str) -> Option<f64> {
    let key = format!("{field}: ");
    let start = text.find(&key)? + key.len();
    let rest = &text[start..];
    let end = rest.find([' ', '}', ',']).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// The direction a container lays its children out in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum FlowDir {
    Right,
    Down,
    Overlay,
}

/// How the walks in one wrapped row line up vertically.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RowAlignText {
    Top,
    Center,
    Bottom,
}

impl RowAlignText {
    fn name(self) -> &'static str {
        match self {
            RowAlignText::Top => "Top",
            RowAlignText::Center => "Center",
            RowAlignText::Bottom => "Bottom",
        }
    }
}

/// A `flow` row as the Props tab reads it: `Flow.Right{row_align:
/// RowAlign.Top wrap: true}`, `Flow.Down`, or the bare `Right` of a
/// hand-written source. The wrap and the row alignment only mean anything
/// for `Right`; they are kept across a change of direction so that going
/// Down and back does not lose them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FlowText {
    dir: FlowDir,
    wrap: bool,
    row_align: RowAlignText,
}

impl FlowText {
    /// Children go left to right and onto a new row when the width runs out.
    fn wraps(&self) -> bool {
        self.dir == FlowDir::Right && self.wrap
    }

    fn with_dir(self, dir: FlowDir) -> Self {
        FlowText { dir, ..self }
    }

    /// The wrap button: on a Right flow it toggles; on any other it turns
    /// the flow into a wrapping Right one, which is what pressing "wrap"
    /// on a Down container can only mean.
    fn toggled_wrap(self) -> Self {
        FlowText { dir: FlowDir::Right, wrap: !self.wraps(), ..self }
    }

    fn with_row_align(self, row_align: RowAlignText) -> Self {
        FlowText { dir: FlowDir::Right, row_align, ..self }
    }

    /// The chunk that sets it: always the whole value. A field of a
    /// variant is not a property, so `flow.wrap: true` is never emitted.
    fn chunk(&self) -> String {
        match self.dir {
            FlowDir::Down => "flow: Flow.Down".to_string(),
            FlowDir::Overlay => "flow: Flow.Overlay".to_string(),
            FlowDir::Right => format!(
                "flow: Flow.Right{{wrap: {} row_align: RowAlign.{}}}",
                self.wrap,
                self.row_align.name()
            ),
        }
    }
}

/// Read a flow row's text, in every spelling it has had: the printed
/// `Flow.Right{row_align: RowAlign.Top wrap: true}`, a bare `Down`, the
/// `Right{wrap: true}` of a source, and the `RightWrap` an older panel
/// wrote. Anything else reads as Down, the commonest container.
fn parse_flow_text(text: &str) -> FlowText {
    let text = text.trim();
    let head = text.split(['{', '(']).next().unwrap_or("").trim();
    let variant = head.rsplit('.').next().unwrap_or("").trim();
    let dir = match variant {
        "Right" | "RightWrap" => FlowDir::Right,
        "Overlay" => FlowDir::Overlay,
        _ => FlowDir::Down,
    };
    let wrap = variant == "RightWrap" || text.contains("wrap: true");
    let row_align = match field_word(text, "row_align") {
        Some("Center") => RowAlignText::Center,
        Some("Bottom") => RowAlignText::Bottom,
        _ => RowAlignText::Top,
    };
    FlowText { dir, wrap, row_align }
}

/// `row_align: RowAlign.Center` out of a printed named variant, as the bare
/// variant word after any enum prefix.
fn field_word<'a>(text: &'a str, field: &str) -> Option<&'a str> {
    let key = format!("{field}: ");
    let start = text.find(&key)? + key.len();
    let rest = &text[start..];
    let end = rest.find([' ', '}', ',']).unwrap_or(rest.len());
    Some(rest[..end].rsplit('.').next().unwrap_or(""))
}

/// The strings out of a printed list -- `["70px" "20%" "1fr"]` -- joined
/// with `sep`; a list that is not strings comes back as it was printed.
fn quoted_list_text(printed: &str, sep: &str) -> String {
    let inner = printed.trim().trim_start_matches('[').trim_end_matches(']').trim();
    if inner.is_empty() {
        return String::new();
    }
    if !inner.starts_with('"') {
        return inner.to_string();
    }
    let mut out = Vec::new();
    let mut rest = inner;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else { break };
        out.push(&after[..end]);
        rest = &after[end + 1..];
    }
    out.join(sep)
}

/// Split a track list on spaces, keeping `minmax(60px, 1fr)` together.
fn split_tracks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for ch in text.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            ' ' | '\t' | ',' if depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A track length as the engine's parser takes it: a number, or one with
/// `px`, `%` or `fr`, or a balanced expression such as `calc(...)`.
fn track_len_ok(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let bare = text
        .strip_suffix("px")
        .or_else(|| text.strip_suffix("fr"))
        .or_else(|| text.strip_suffix('%'))
        .unwrap_or(text);
    if bare.trim().parse::<f64>().is_ok_and(|v| v >= 0.0) {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    ["calc(", "min(", "max(", "clamp("].iter().any(|f| lower.starts_with(f))
        && lower.ends_with(')')
        && lower.matches('(').count() == lower.matches(')').count()
}

/// One track as the engine's parser takes it: a length, `minmax(a, b)`, or
/// `repeat(n | auto-fill | auto-fit, minmax(a, b))`.
fn track_ok(text: &str) -> bool {
    let text = text.trim();
    if let Some(body) = text.strip_prefix("minmax(").and_then(|b| b.strip_suffix(')')) {
        let parts: Vec<&str> = body.splitn(2, ',').collect();
        return parts.len() == 2 && parts.iter().all(|p| track_len_ok(p));
    }
    if let Some(body) = text.strip_prefix("repeat(").and_then(|b| b.strip_suffix(')')) {
        let Some((count, segment)) = body.split_once(',') else { return false };
        let count = count.trim();
        let count_ok = count == "auto-fill" || count == "auto-fit" || count.parse::<u32>().is_ok();
        return count_ok && segment.trim().starts_with("minmax(") && track_ok(segment.trim());
    }
    track_len_ok(text)
}

/// What was typed into a tracks field, as the list the engine takes; a
/// half-typed track is not sent, and no tracks at all is an empty list.
fn tracks_chunk(prop: &str, typed: &str) -> Option<String> {
    let tracks = split_tracks(typed);
    if !tracks.iter().all(|t| track_ok(t)) {
        return None;
    }
    let quoted: Vec<String> = tracks.iter().map(|t| format!("\"{}\"", t.trim())).collect();
    Some(format!("{prop}: [{}]", quoted.join(" ")))
}

/// What was typed into the areas field: rows separated by `/`, each a row
/// of names with `.` for an empty cell. Rows of unequal length are a row
/// still being typed, and are not sent.
fn areas_chunk(typed: &str) -> Option<String> {
    let rows: Vec<String> = typed
        .split('/')
        .map(|row| row.trim().replace('"', ""))
        .filter(|row| !row.is_empty())
        .collect();
    let width = rows.first().map(|row| row.split_whitespace().count());
    if rows.iter().any(|row| Some(row.split_whitespace().count()) != width) {
        return None;
    }
    let quoted: Vec<String> = rows.iter().map(|row| format!("\"{row}\"")).collect();
    Some(format!("areas: [{}]", quoted.join(" ")))
}

/// Where a child sits in its Grid, as the rows say: 0 is wherever the
/// fill order puts it.
#[derive(Clone, Debug, Default, PartialEq)]
struct CellText {
    col: u32,
    row: u32,
    col_span: u32,
    row_span: u32,
    area: String,
}

impl CellText {
    /// The chunk that places it; a cell that says nothing is no cell.
    fn chunk(&self) -> String {
        if self.col == 0 && self.row == 0 && self.col_span == 0 && self.row_span == 0 && self.area.is_empty() {
            return "cell: nil".to_string();
        }
        let area = if self.area.is_empty() { "nil".to_string() } else { format!("@{}", self.area) };
        format!(
            "cell: CellPlacement{{col: {} row: {} col_span: {} row_span: {} area: {area}}}",
            self.col, self.row, self.col_span, self.row_span
        )
    }
}

/// A container name as the chunk that sets it: an id, so letters, digits
/// and underscores, not starting with a digit; anything else is not sent.
fn container_chunk(typed: &str) -> Option<String> {
    let name = typed.trim().trim_start_matches('@');
    let valid = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    valid.then(|| format!("container_id: @{name}"))
}

/// `vec2f(10 20)` as printed, or `vec2(10, 20)` as written, to its numbers.
fn parse_vec2_text(text: &str) -> Option<(f64, f64)> {
    let inner = text.split('(').nth(1)?.trim_end_matches(')');
    let mut parts = inner.split([' ', ',']).filter(|p| !p.is_empty());
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    Some((x, y))
}

/// What a person typed into a size field, as the chunk that sets it. A bare
/// word is a mode, a number is points, a percent or a viewport unit or a
/// function is the CSS spelling and goes in quotes so the engine's string
/// parser sees it. Text that reads as none of those -- the half of a word
/// still being typed -- is not sent.
fn size_chunk(axis: &str, typed: &str) -> Option<String> {
    let typed = typed.trim().trim_matches('"').trim();
    match typed.to_ascii_lowercase().as_str() {
        "fill" => return Some(format!("{axis}: Fill")),
        "fit" => return Some(format!("{axis}: Fit")),
        _ => {}
    }
    match parse_size_text(typed) {
        SizeText::Fixed(v) => Some(format!("{axis}: {}", fmt_f64(v))),
        SizeText::Rel(text) | SizeText::Expr(text) => Some(format!("{axis}: \"{text}\"")),
        SizeText::Fill(_) => Some(format!("{axis}: Fill")),
        SizeText::Fit => None,
    }
}

fn fmt_value(heap: &ScriptHeap, value: ScriptValue, depth: usize) -> Option<String> {
    if let Some(text) = fmt_scalar(heap, value) {
        return Some(text);
    }
    if let Some(array) = value.as_array() {
        let mut out = String::from("[");
        let len = heap.array_len(array).min(8);
        for index in 0..len {
            if index > 0 {
                out.push(' ');
            }
            let item = heap.array_index(array, index, NoTrap);
            match fmt_value(heap, item, depth.saturating_sub(1)) {
                Some(text) => out.push_str(&text),
                None => out.push('?'),
            }
        }
        out.push(']');
        return Some(out);
    }
    if let Some(obj) = value.as_object() {
        if heap.as_fn(obj).is_some() {
            return None; // functions are not properties
        }
        // `instance(v)` / `uniform(v)` shader-io wrappers carry the value in
        // their proto slot — unwrap so shader inputs read as plain values.
        if heap.as_shader_io(obj).is_some() {
            let inner = heap.proto(obj);
            if inner.is_nil() {
                return None; // texture/buffer declarations, not values
            }
            return fmt_value(heap, inner, depth);
        }
        // A derived enum prints as one whole value, never as its fields.
        if let Some(text) = fmt_enum(heap, obj, EnumFmt::Full) {
            return Some(text);
        }
        if depth == 0 {
            return Some("{..}".to_string());
        }
        let mut keys: Vec<LiveId> = Vec::new();
        collect_prop_keys(heap, obj, &mut keys);
        let mut out = String::from("{");
        let mut first = true;
        for key in keys.into_iter().take(24) {
            let value = heap.value(obj, key.into(), NoTrap);
            let Some(text) = fmt_value(heap, value, depth - 1) else {
                continue;
            };
            if !first {
                out.push(' ');
            }
            first = false;
            out.push_str(&live_id_token(key));
            out.push_str(": ");
            out.push_str(&text);
        }
        out.push('}');
        return Some(out);
    }
    None
}

/// The union of map keys over the object and its proto chain, leaf first —
/// exactly the widget's live surface: what was applied plus every default
/// the type registered.
fn collect_prop_keys(heap: &ScriptHeap, obj: ScriptObject, keys: &mut Vec<LiveId>) {
    let mut ptr = obj;
    let mut hops = 0;
    loop {
        heap.map_ref(ptr).iter().for_each(|(key, _map_value)| {
            if let Some(id) = key.as_id() {
                if !keys.contains(&id) {
                    let name = live_id_token(id);
                    if !name.starts_with("__") && !skip_key(&name) {
                        keys.push(id);
                    }
                }
            }
        });
        match heap.proto(ptr).as_object() {
            Some(next) => {
                ptr = next;
                hops += 1;
                if hops > 24 {
                    break;
                }
            }
            None => break,
        }
    }
}

/// A value rendering that carries no information for the sidebar or diff.
fn is_noise(text: &str) -> bool {
    matches!(text, "{}" | "{..}" | "null")
}

/// Flat `name -> value` reflection of a widget's CURRENT state, one level of
/// nesting expanded with dotted names (`draw_bg.color`). Values come from
/// `script_to_value` — the Rust fields serialized back to script — so runtime
/// applies are visible; the `#[source]` object's own map (what the DSL
/// explicitly applied) supplies the `set` flag. This is both the sidebar's
/// data and the before/after capture the diff log works from. Part of the
/// [`crate::reflect`] surface: the same read a catalogue app's Docs and
/// Controls panels build on.
pub fn reflect_flat(cx: &mut Cx, widget: &WidgetRef) -> Vec<(String, String, bool)> {
    cx.with_vm(|vm| {
        // Serializing a widget back to script trips harmless type-check
        // complaints on fn-ref fields (`on_click` serializes to a value its
        // own type check rejects). Reflection is read-only — capture and
        // drop them instead of spamming the log on every state read.
        vm.bx.captured_errors = Some(Vec::new());
        let current = widget.current_to_value(vm);
        let _ = vm.take_errors();
        let Some(current_obj) = current.as_object() else {
            return Vec::new();
        };
        let source = widget.script_source();
        let heap = &vm.bx.heap;
        let own: Vec<LiveId> = if source == ScriptObject::ZERO {
            Vec::new()
        } else {
            heap.map_ref(source)
                .iter()
                .filter_map(|(key, _)| key.as_id())
                .collect()
        };
        let mut out = Vec::new();
        // Only the object's OWN map: script_to_value wrote every live field
        // explicitly, the proto behind it is just the type object.
        let keys: Vec<LiveId> = heap
            .map_ref(current_obj)
            .iter()
            .filter_map(|(key, _)| key.as_id())
            .filter(|id| {
                let name = live_id_token(*id);
                !name.starts_with("__") && !skip_key(&name)
            })
            .collect();
        for key in keys {
            let value = heap.value(current_obj, key.into(), NoTrap);
            let is_set = own.contains(&key);
            let name = live_id_token(key);
            if let Some(obj) = value.as_object() {
                if heap.as_fn(obj).is_some() {
                    continue;
                }
                // An enum is one row, whole -- `width: Size.Fill{weight: 100}`
                // -- and is never dotted into `width.weight`: a field of a
                // variant is not a property a person can set on its own.
                if enum_info(heap, obj).is_some() {
                    if let Some(text) = fmt_enum(heap, obj, EnumFmt::Full) {
                        out.push((name, text, is_set));
                    }
                    continue;
                }
                // One level of dotted expansion for typed sub-structs
                // (draw_bg.color, padding.left ...), own map only.
                let sub_keys: Vec<LiveId> = heap
                    .map_ref(obj)
                    .iter()
                    .filter_map(|(sub_key, _)| sub_key.as_id())
                    .filter(|id| {
                        let name = live_id_token(*id);
                        !name.starts_with("__") && !is_shader_plumbing(&name)
                    })
                    .collect();
                let had_subs = !sub_keys.is_empty();
                let mut wrote_sub = false;
                for sub_key in sub_keys.into_iter().take(200) {
                    let sub_value = heap.value(obj, sub_key.into(), NoTrap);
                    if sub_value.as_object().is_some_and(|o| heap.as_fn(o).is_some()) {
                        continue;
                    }
                    if let Some(text) = fmt_value(heap, sub_value, 1) {
                        if is_noise(&text) {
                            continue;
                        }
                        out.push((format!("{name}.{}", live_id_token(sub_key)), text, is_set));
                        wrote_sub = true;
                    }
                }
                if !wrote_sub && !had_subs {
                    if let Some(text) = fmt_value(heap, value, 1) {
                        if !is_noise(&text) {
                            out.push((name, text, is_set));
                        }
                    }
                }
            } else if let Some(text) = fmt_value(heap, value, 1) {
                if !is_noise(&text) || is_set {
                    out.push((name, text, is_set));
                }
            }
            if out.len() > 400 {
                break;
            }
        }
        // Second pass: the type's shader-input registry. Instance/uniform
        // inputs declared in the DSL (`border_radius: instance(2.0)`) are
        // not Rust fields, so `script_to_value` never sees them — but they
        // live in the source object's proto chain. Union them in (source
        // values are the pre-tweak truth; tweaks to them land in the diff).
        if source != ScriptObject::ZERO {
            let mut keys = Vec::new();
            collect_prop_keys(heap, source, &mut keys);
            for key in keys {
                let name = live_id_token(key);
                if is_shader_plumbing(&name) {
                    continue;
                }
                let value = heap.value(source, key.into(), NoTrap);
                if let Some(obj) = value.as_object() {
                    if heap.as_fn(obj).is_some() {
                        continue;
                    }
                    if enum_info(heap, obj).is_some() {
                        if !out.iter().any(|(existing, _, _)| *existing == name) {
                            if let Some(text) = fmt_enum(heap, obj, EnumFmt::Full) {
                                out.push((name, text, false));
                            }
                        }
                        continue;
                    }
                    // The live value answered for this key with nothing --
                    // `cell: null` after it was cleared -- so the source's
                    // fields under it are what WAS, not what is. A live
                    // object that dumped whole still gets its declared
                    // inputs read out of the source (a draw layer's
                    // `color`, `border_size`), which only live there.
                    if out.iter().any(|(existing, text, _)| *existing == name && text == "null") {
                        continue;
                    }
                    let mut sub_keys = Vec::new();
                    collect_prop_keys(heap, obj, &mut sub_keys);
                    sub_keys.retain(|id| {
                        let name = live_id_token(*id);
                        !name.starts_with("__") && !is_shader_plumbing(&name)
                    });
                    for sub_key in sub_keys.into_iter().take(200) {
                        let sub_name = live_id_token(sub_key);
                        let dotted = format!("{name}.{sub_name}");
                        if out.iter().any(|(existing, _, _)| *existing == dotted) {
                            continue;
                        }
                        let sub_value = heap.value(obj, sub_key.into(), NoTrap);
                        if sub_value
                            .as_object()
                            .is_some_and(|o| heap.as_fn(o).is_some())
                        {
                            continue;
                        }
                        if let Some(text) = fmt_value(heap, sub_value, 1) {
                            if !is_noise(&text) {
                                out.push((dotted, text, false));
                            }
                        }
                    }
                } else {
                    if out.iter().any(|(existing, _, _)| *existing == name) {
                        continue;
                    }
                    if let Some(text) = fmt_value(heap, value, 1) {
                        if !is_noise(&text) {
                            out.push((name, text, false));
                        }
                    }
                }
                if out.len() > 400 {
                    break;
                }
            }
        }
        out
    })
}

// ---------------------------------------------------------------------------
// live-load — evaluate a splash chunk and apply it onto the one selected
// instance through the ordinary apply machinery, then a full redraw/relayout.
// ---------------------------------------------------------------------------


/// Apply `sel`'s draw-layer SOURCE object + the session-ledger overlay onto
/// a preview quad — the swatch then renders with the selection's actual
/// compiled shader (fn-hash cache) at its live-tweaked values. Quad-family
/// layers only; anything else leaves the preview untouched.
/// Re-indent a fn's source for the view: drop the common leading indent and
/// turn every remaining 4-space (or tab) step into 2 spaces — "much less
/// aggressive indenting like 2 spaces per tab".
fn reindent_two(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            let indent: usize = l.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum();
            let rest = l.trim_start_matches([' ', '\t']);
            let level = indent.saturating_sub(common) / 4;
            format!("{}{}", "  ".repeat(level), rest)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Remember `name: fn() {…}` entries of an applied `<layer> +: { … }` chunk
/// so the source view shows what runs, not what the file says.
fn record_fn_overrides(uid: u64, chunk: &str) {
    let chunk = chunk.trim();
    let Some(open) = chunk.find("+:") else { return };
    let layer = chunk[..open].trim().to_string();
    let Some(body) = chunk[open..].find('{').map(|i| &chunk[open + i + 1..]) else { return };
    let body = body.trim_end().trim_end_matches('}');
    let mut s = session().lock().unwrap();
    s.fn_override_gen += 1;
    for name in ["pixel", "vertex"] {
        if let Some(seg) = body.split(&format!("{name}:")).nth(1) {
            let seg = format!("{name}:{seg}");
            s.fn_overrides.insert((uid, layer.clone(), name.to_string()), reindent_two(seg.trim_end()));
        }
    }
}

/// Ctrl+Enter in the source view: apply the fn as edited to the pinned
/// widget's layer, live — no AI needed for a hand edit.
fn apply_fn_edit(cx: &mut Cx, tweaker: &mut Tweaker, text: &str) -> Result<(), String> {
    let Some(sel) = session().lock().unwrap().pinned.clone() else {
        return Err("nothing pinned".to_string());
    };
    let layer = tweaker.vibe_layer.clone().unwrap_or_else(|| "draw_bg".to_string());
    // Strip the `// name — loc` header lines the view adds; what is left is
    // one or more `name: fn() { … }` entries — a valid layer body.
    let body: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("// "))
        .collect::<Vec<_>>()
        .join("\n");
    let chunk = format!("{layer} +: {{\n{body}\n}}");
    match apply_splash_chunk(cx, &cx.widget_tree().widget(WidgetUid(sel.uid)), &sel.path, &chunk, "editor") {
        Ok(_) => {
            log!("TWEAK editor applied {} fn edit to {}", layer, sel.path);
            let mut s = session().lock().unwrap();
            for (name, _, _) in &tweaker.vibe_fn_sources {
                // Remember the applied text per fn so the view shows it.
                if let Some(seg) = body.split(&format!("{name}:")).nth(1) {
                    let seg = format!("{name}:{seg}");
                    s.fn_overrides.insert((sel.uid, layer.clone(), name.clone()), seg.trim_end().to_string());
                }
            }
            Ok(())
        }
        Err(error) => {
            log!("TWEAK editor apply failed: {error}");
            Err(error)
        }
    }
}

/// The script-defined shader fns of a widget's draw layer — `pixel` and
/// `vertex` when the layer (or a proto in its chain) sets one — as
/// (name, "file:line", source text). What the Shader tab shows under the
/// well and what a code-only prompt rewrites.
fn layer_fn_sources(cx: &mut Cx, widget: &WidgetRef, layer: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return out;
    }
    let layer_id = LiveId::from_str(layer);
    cx.with_vm(|vm| {
        let value = vm.bx.heap.value(source, layer_id.into(), NoTrap);
        if value.as_object().is_none() {
            return;
        }
        // Closest level first: the fn the widget actually runs is the
        // nearest definition up the layer's construction chain (a `+:`
        // merge in the widget's script, else the draw type's own).
        let chain = vm.construction_chain(value);
        for (name, key) in [("pixel", id!(pixel)), ("vertex", id!(vertex))] {
            for lvl in &chain {
                if !lvl.own_keys.contains(&key) {
                    continue;
                }
                let f = vm.bx.heap.value(lvl.object, key.into(), NoTrap);
                let Some(fobj) = f.as_object() else { break };
                let Some(makepad_script::ScriptFnPtr::Script(ip)) = vm.bx.heap.as_fn(fobj) else {
                    break;
                };
                if let Some((loc, text)) = vm.bx.code.fn_source_text(ip) {
                    let base = loc.file.rsplit('/').next().unwrap_or(&loc.file).to_string();
                    out.push((name.to_string(), format!("{base}:{}", loc.line), text));
                }
                break;
            }
        }
    });
    out
}

/// One animator track (hover, down, focus, disabled…) of a widget, read
/// from its script cascade, ready to POSE the layer's instance slice: the
/// values the off and on states apply to this layer, per shader input.
#[derive(Clone)]
pub struct StateTrack {
    pub group: String,
    fields: Vec<StateField>,
    /// Into `on` from `off` (the track's own `from` map, `all` fallback).
    on_play: Play,
    on_ease: AnimEase,
    /// Back into `off`.
    off_play: Play,
    off_ease: AnimEase,
}

#[derive(Clone)]
struct StateField {
    id: LiveId,
    /// Slot values of the pose; None = whatever the live widget has.
    off: Option<Vec<f32>>,
    on: Option<Vec<f32>>,
}

impl StateTrack {
    /// Write the pose between off (0) and on (1) over the mirrored
    /// instance, by the shader's own instance layout.
    fn pose(&self, cx: &Cx, mirror: &mut MaterialMirror, mix: f32) {
        let Some(shader_id) = mirror.draw_vars.draw_shader_id else { return };
        let sh = &cx.draw_shaders[shader_id.index];
        for field in &self.fields {
            let Some(input) = sh.mapping.instances.inputs.iter().find(|i| i.id == field.id) else {
                continue;
            };
            for slot in 0..input.slots {
                let at = input.offset + slot;
                if at >= mirror.instance.len() {
                    break;
                }
                let live = mirror.instance[at];
                let a = field.off.as_ref().and_then(|v| v.get(slot).copied()).unwrap_or(live);
                let b = field.on.as_ref().and_then(|v| v.get(slot).copied()).unwrap_or(live);
                mirror.instance[at] = a + (b - a) * mix;
            }
        }
    }
}

/// Where a track is in its off→on→off cycle at time `t` (seconds since
/// the cycle began): the real transition (duration + ease from the
/// animator, a Snap jumps) each way, each pose held for a beat.
fn track_mix(track: &StateTrack, t: f64) -> f32 {
    const HOLD: f64 = 0.7;
    const MIN: f64 = 0.15;
    let on_d = play_duration(track.on_play).max(MIN);
    let off_d = play_duration(track.off_play).max(MIN);
    let cycle = on_d + HOLD + off_d + HOLD;
    let p = t.rem_euclid(cycle);
    if p < on_d {
        if matches!(track.on_play, Play::Snap) { 1.0 } else { track.on_ease.map(p / on_d) as f32 }
    } else if p < on_d + HOLD {
        1.0
    } else if p < on_d + HOLD + off_d {
        if matches!(track.off_play, Play::Snap) {
            0.0
        } else {
            1.0 - track.off_ease.map((p - on_d - HOLD) / off_d) as f32
        }
    } else {
        0.0
    }
}

fn play_duration(play: Play) -> f64 {
    match play {
        Play::Snap => 0.0,
        Play::Forward { duration }
        | Play::Reverse { duration, .. }
        | Play::Loop { duration, .. }
        | Play::ReverseLoop { duration, .. }
        | Play::BounceLoop { duration, .. } => duration,
    }
}

/// A state's `apply` values for one layer, as shader slots: numbers are
/// one slot, colours four (rgba 0..1), bools one.
fn pose_values(vm: &mut ScriptVm, state: &AnimatorState, layer_id: LiveId) -> Vec<(LiveId, Vec<f32>)> {
    let mut out = Vec::new();
    let Some(apply) = state.apply else { return out };
    let layer_value = vm.bx.heap.value(apply, layer_id.into(), NoTrap);
    let Some(layer_obj) = layer_value.as_object() else { return out };
    let mut keys: Vec<LiveId> = Vec::new();
    for lvl in vm.construction_chain(layer_value) {
        for key in &lvl.own_keys {
            if !keys.contains(key) {
                keys.push(*key);
            }
        }
    }
    for key in keys {
        let mut value = vm.bx.heap.value(layer_obj, key.into(), NoTrap);
        // `snap(1.0)` / `instance(x)` wrap the value in an object keyed
        // `value`; the pose is the wrapped scalar.
        if value.as_color().is_none() && value.as_bool().is_none() && value.as_number().is_none() {
            if let Some(obj) = value.as_object() {
                let inner = vm.bx.heap.value(obj, id!(value).into(), NoTrap);
                if !inner.is_nil() {
                    value = inner;
                }
            }
        }
        let slots = if let Some(c) = value.as_color() {
            vec![
                ((c >> 24) & 0xff) as f32 / 255.0,
                ((c >> 16) & 0xff) as f32 / 255.0,
                ((c >> 8) & 0xff) as f32 / 255.0,
                (c & 0xff) as f32 / 255.0,
            ]
        } else if let Some(b) = value.as_bool() {
            vec![if b { 1.0 } else { 0.0 }]
        } else if let Some(n) = value.as_number() {
            vec![n as f32]
        } else {
            continue;
        };
        out.push((key, slots));
    }
    out
}

/// The widget's animator tracks that touch `layer`, from its script
/// cascade (the instance's own animator, else its type's): per group the
/// default state is "off" and the first other state is "on".
fn animator_tracks(cx: &mut Cx, widget: &WidgetRef, layer: &str) -> Vec<StateTrack> {
    let mut out = Vec::new();
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return out;
    }
    let layer_id = LiveId::from_str(layer);
    cx.with_vm(|vm| {
        let anim = vm.bx.heap.value(source, id!(animator).into(), NoTrap);
        let Some(anim_obj) = anim.as_object() else { return };
        let mut groups: Vec<LiveId> = Vec::new();
        for lvl in vm.construction_chain(anim) {
            for key in &lvl.own_keys {
                if !groups.contains(key) {
                    groups.push(*key);
                }
            }
        }
        for group in groups {
            let group_value = vm.bx.heap.value(anim_obj, group.into(), NoTrap);
            let Some(group_obj) = group_value.as_object() else { continue };
            let mut default = LiveId(0);
            let mut states: Vec<LiveId> = Vec::new();
            for lvl in vm.construction_chain(group_value) {
                for key in &lvl.own_keys {
                    if *key == id!(default) {
                        continue;
                    }
                    if !states.contains(key) {
                        states.push(*key);
                    }
                }
            }
            if let Some(id) = vm.bx.heap.value(group_obj, id!(default).into(), NoTrap).as_id() {
                default = id;
            }
            if states.is_empty() {
                continue;
            }
            let off = if states.contains(&default) { default } else { states[0] };
            let Some(on) = states.iter().copied().find(|s| *s != off) else { continue };
            let off_state = AnimatorState::script_from_value(vm, vm.bx.heap.value(group_obj, off.into(), NoTrap));
            let on_state = AnimatorState::script_from_value(vm, vm.bx.heap.value(group_obj, on.into(), NoTrap));
            let off_values = pose_values(vm, &off_state, layer_id);
            let on_values = pose_values(vm, &on_state, layer_id);
            if off_values.is_empty() && on_values.is_empty() {
                // This track never touches the layer (a text-only state).
                continue;
            }
            let mut fields: Vec<StateField> = Vec::new();
            for (id, v) in &off_values {
                fields.push(StateField { id: *id, off: Some(v.clone()), on: None });
            }
            for (id, v) in &on_values {
                match fields.iter_mut().find(|f| f.id == *id) {
                    Some(f) => f.on = Some(v.clone()),
                    None => fields.push(StateField { id: *id, off: None, on: Some(v.clone()) }),
                }
            }
            let on_play = on_state
                .from
                .get(&off)
                .or_else(|| on_state.from.get(&id!(all)))
                .copied()
                .unwrap_or(Play::Forward { duration: 0.3 });
            let off_play = off_state
                .from
                .get(&on)
                .or_else(|| off_state.from.get(&id!(all)))
                .copied()
                .unwrap_or(Play::Forward { duration: 0.3 });
            out.push(StateTrack {
                group: format!("{}", live_id_token(group)),
                fields,
                on_play,
                on_ease: on_state.ease.unwrap_or(AnimEase::Linear),
                off_play,
                off_ease: off_state.ease.unwrap_or(AnimEase::Linear),
            });
        }
    });
    out
}

/// One live draw call's inputs, copied from a widget's area: what the
/// material swatch draws so it shows exactly what the widget shows.
struct MaterialMirror {
    /// The swatch's own DrawVars re-pointed at the widget's shader, with
    /// the widget's draw call uniforms, textures and geometry copied in.
    draw_vars: DrawVars,
    instance: Vec<f32>,
    rect_pos: Option<usize>,
    rect_size: Option<usize>,
    draw_clip: Option<usize>,
}

impl MaterialMirror {
    /// The widget's own rect size, from the copied instance.
    fn native_size(&self) -> Vec2d {
        match self.rect_size {
            Some(rs) => dvec2(self.instance[rs] as f64, self.instance[rs + 1] as f64),
            None => dvec2(64.0, 24.0),
        }
    }

    fn draw(&self, cx: &mut Cx2d, rect: Rect, clip: Option<(Vec2d, Vec2d)>) {
        let draw_vars = &self.draw_vars;
        let Some(mut many) = cx.begin_many_instances(draw_vars) else {
            return;
        };
        let mut inst = self.instance.clone();
        if let Some(rp) = self.rect_pos {
            inst[rp] = rect.pos.x as f32;
            inst[rp + 1] = rect.pos.y as f32;
        }
        if let Some(rs) = self.rect_size {
            inst[rs] = rect.size.x as f32;
            inst[rs + 1] = rect.size.y as f32;
        }
        if let Some(dc) = self.draw_clip {
            // The well is its own draw list with a magnifier transform, so
            // nothing upstream clips it: the caller passes the scroll
            // viewport's clip mapped into this (pre-transform) space, and a
            // well scrolling under the panel header is cut off exactly like
            // a text row.
            let (min, max) = clip.unwrap_or((rect.pos, rect.pos + rect.size));
            inst[dc] = min.x.max(rect.pos.x) as f32;
            inst[dc + 1] = min.y.max(rect.pos.y) as f32;
            inst[dc + 2] = max.x.min(rect.pos.x + rect.size.x) as f32;
            inst[dc + 3] = max.y.min(rect.pos.y + rect.size.y) as f32;
        }
        many.instances.extend_from_slice(&inst);
        let area = cx.end_many_instances(many);
        if crate::makepad_platform::makepad_error_log::trace_enabled("tweak") {
            if let Area::Instance(ia) = area {
                if let Some(dc) = cx.draw_lists[ia.draw_list_id].draw_items[ia.draw_item_id].draw_call() {
                    trace!("tweak", "swatch copy shader={:?} inst={:?} uniforms[..24]={:?}", dc.draw_shader_id, &inst, &dc.dyn_uniforms[..24]);
                }
            }
        }
    }
}

/// Read a widget's live draw call through its area: the first instance's
/// values plus the call's shader, geometry, uniforms and textures.
fn capture_material_mirror(cx: &Cx, widget: &WidgetRef, area: Area, base: &DrawVars) -> Option<MaterialMirror> {
    let Area::Instance(inst) = area else {
        return None;
    };
    if inst.instance_count == 0 {
        return None;
    }
    let draw_list = &cx.draw_lists[inst.draw_list_id];
    let draw_item = &draw_list.draw_items[inst.draw_item_id];
    let draw_call = draw_item.draw_call()?;
    let buf = draw_item.instances.as_ref()?;
    let sh = &cx.draw_shaders[draw_call.draw_shader_id.index];
    let stride = sh.mapping.instances.total_slots;
    if stride == 0 || inst.instance_offset + stride > buf.len() {
        return None;
    }
    if crate::makepad_platform::makepad_error_log::trace_enabled("tweak") {
        trace!(
            "tweak",
            "swatch source uid={} shader={:?} stride={} inst={:?} uniforms[..24]={:?}",
            widget.widget_uid().0,
            draw_call.draw_shader_id,
            stride,
            &buf[inst.instance_offset..inst.instance_offset + stride],
            &draw_call.dyn_uniforms[..24]
        );
    }
    let mut draw_vars = base.clone();
    draw_vars.area = Area::Empty;
    draw_vars.draw_shader_id = Some(draw_call.draw_shader_id);
    draw_vars.geometry_id = draw_call.geometry_id;
    draw_vars.options = draw_call.options.clone();
    draw_vars.dyn_uniforms = draw_call.dyn_uniforms;
    draw_vars.texture_slots = draw_call.texture_slots.clone();
    draw_vars.uniform_buffer_slots = draw_call.uniform_buffer_slots.clone();
    Some(MaterialMirror {
        draw_vars,
        instance: buf[inst.instance_offset..inst.instance_offset + stride].to_vec(),
        rect_pos: sh.mapping.rect_pos,
        rect_size: sh.mapping.rect_size,
        draw_clip: sh.mapping.draw_clip,
    })
}


/// Feed the undo stack from one applied ledger entry. Consecutive applies
/// to the same prop merge while the gesture is open (a scrub = one step);
/// any new user gesture clears the redo branch. Undo/redo replays pass
/// origin "undo"/"redo" and are not tracked.
pub(crate) fn track_undo(s: &mut TweakSession, entry: &TweakDiffEntry) {
    s.redo.clear();
    if s.undo_open {
        if let Some(UndoStep::Value { path, prop, new, .. }) = s.undo.last_mut() {
            if *path == entry.path && *prop == entry.prop {
                *new = entry.new.clone();
                return;
            }
        }
    }
    s.undo.push(UndoStep::Value {
        path: entry.path.clone(),
        prop: entry.prop.clone(),
        old: entry.old.clone(),
        new: entry.new.clone(),
        seq_start: entry.seq,
    });
    s.undo_open = true;
}

/// One level of the selection's construction chain, display-ready.
/// Built from `vm.construction_chain` over the widget's `#[source]` object:
/// the proto chain of `made_at` ips, each resolved to a source location and
/// its `///` docs (see platform/script/src/docs.rs).
#[derive(Clone)]
struct CascadeLevel {
    /// "button.rs:52" (basename:line), or "native" for Rust-built levels.
    loc: String,
    /// Full file path for click-through / the AI ("" for native levels).
    file: String,
    line: u32,
    /// `///` doc attached to the level's object literal.
    doc: Option<String>,
    /// `///` docs attached to fields of that literal.
    field_docs: Vec<(String, String)>,
    /// Keys the level sets itself; true = overridden by a closer level.
    sets: Vec<(String, bool)>,
}

/// Doc-channel text per row prop: field docs from the widget's own
/// construction chain plus one dotted level into its object-valued keys
/// (draw layers, typed sub-structs) — `draw_bg.border_size` finds the
/// `/** bevel border thickness 0..4 step 0.5 */` written inside the
/// draw_bg literal. Closest level wins. Feeds tooltips and the
/// hints->scrubber wiring (`parse_doc_hint`).
/// A reflected value that is a structured type gets a typed editor rather
/// than a `{..}`/`vec2f(..)` text dump. Determined from the dump text —
/// no reflection change — so it stays a pure display-layer concern.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StructKind {
    None,
    Vec2,
    Vec3,
    Vec4,
    Inset,
    Metrics,
    /// Recognized as structured but with no editor yet (a big nested
    /// struct like a full text_style): shown collapsed, never dumped.
    NoEditor,
}

/// The hover pulse: every drawn value equal to a theme colour — dynamic
/// uniforms and colour instances across every draw call — is scaled in
/// brightness in place (CPU-side buffers, dirty flags set, uploaded next
/// frame). No ledger, no apply: stopping just redraws all, which rebuilds
/// every buffer from the widgets — restore is the ordinary draw path.
/// The pulsed form of a colour at mix amount `m` (0 = the true colour).
/// A brightness multiply is invisible on black or near-transparent theme
/// colours, so the pulse mixes rgb toward the contrast tone (white for
/// dark colours, black for light ones) and lifts alpha toward opaque —
/// visible for any colour, and exactly invertible via the same function.
fn pulse_tone(target: [f32; 4], m: f32) -> [f32; 4] {
    let lum = 0.299 * target[0] + 0.587 * target[1] + 0.114 * target[2];
    let tone = if lum < 0.5 { 1.0 } else { 0.0 };
    [
        target[0] + (tone - target[0]) * m,
        target[1] + (tone - target[1]) * m,
        target[2] + (tone - target[2]) * m,
        target[3] + (1.0 - target[3]) * m * 0.85,
    ]
}

/// One colour slot the pulse has written. Theme colours reach the GPU
/// four ways: per-instance values, a draw call's dyn uniforms, a shader
/// referencing the theme in its code (a per-shader scope uniform), and a
/// pass clear colour (the window background).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PulseSlot {
    Inst { list: DrawListId, item: usize, at: usize },
    Uni { list: DrawListId, item: usize, at: usize },
    Scope { shader: usize, at: usize },
    Clear { pass: DrawPassId },
}

/// The live theme pulse: the colour, the current mix, the last value
/// written, and the ledger of every slot holding it. Slots are re-toned
/// by identity, never by value, so no other colour can be caught.
struct PulseState {
    target: [f32; 4],
    m: f32,
    last: [f32; 4],
    /// A theme edit: every slot holds this value instead of a pulse tone.
    fixed: Option<[f32; 4]>,
    ledger: Vec<PulseSlot>,
}

impl PulseState {
    fn new(rgba: u32) -> Self {
        let target = [
            ((rgba >> 24) & 0xff) as f32 / 255.0,
            ((rgba >> 16) & 0xff) as f32 / 255.0,
            ((rgba >> 8) & 0xff) as f32 / 255.0,
            (rgba & 0xff) as f32 / 255.0,
        ];
        Self { target, m: 0.0, last: target, fixed: None, ledger: Vec::new() }
    }
}

fn pulse_close(v: &[f32], w: [f32; 4]) -> bool {
    (v[0] - w[0]).abs() < 0.004
        && (v[1] - w[1]).abs() < 0.004
        && (v[2] - w[2]).abs() < 0.004
        && (v[3] - w[3]).abs() < 0.004
}

fn pulse_slot_get(cx: &Cx, s: PulseSlot) -> Option<[f32; 4]> {
    let v: [f32; 4] = match s {
        PulseSlot::Inst { list, item, at } => {
            let items = &cx.draw_lists[list].draw_items;
            if item >= items.len() {
                return None;
            }
            let b = items[item].instances.as_ref()?.get(at..at + 4)?;
            [b[0], b[1], b[2], b[3]]
        }
        PulseSlot::Uni { list, item, at } => {
            let items = &cx.draw_lists[list].draw_items;
            if item >= items.len() {
                return None;
            }
            let b = items[item].draw_call()?.dyn_uniforms.get(at..at + 4)?;
            [b[0], b[1], b[2], b[3]]
        }
        PulseSlot::Scope { shader, at } => {
            let b = cx.draw_shaders.shaders.get(shader)?.mapping.scope_uniforms_buf.get(at..at + 4)?;
            [b[0], b[1], b[2], b[3]]
        }
        PulseSlot::Clear { pass } => {
            let c = cx.passes[pass].clear_color;
            [c.x, c.y, c.z, c.w]
        }
    };
    Some(v)
}

fn pulse_slot_set(cx: &mut Cx, s: PulseSlot, v: [f32; 4]) {
    match s {
        PulseSlot::Inst { list, item, at } => {
            let items = &mut cx.draw_lists[list].draw_items;
            if item >= items.len() {
                return;
            }
            let it = &mut items[item];
            if let Some(dst) = it.instances.as_mut().and_then(|b| b.get_mut(at..at + 4)) {
                dst.copy_from_slice(&v);
            }
            if let Some(call) = it.kind.draw_call_mut() {
                call.instance_dirty = true;
            }
        }
        PulseSlot::Uni { list, item, at } => {
            let uniforms_gen = cx.next_uniform_gen();
            let items = &mut cx.draw_lists[list].draw_items;
            if item >= items.len() {
                return;
            }
            if let Some(call) = items[item].kind.draw_call_mut() {
                if let Some(dst) = call.dyn_uniforms.get_mut(at..at + 4) {
                    dst.copy_from_slice(&v);
                    call.mark_uniforms_dirty(uniforms_gen);
                }
            }
        }
        PulseSlot::Scope { shader, at } => {
            let uniforms_gen = cx.next_uniform_gen();
            if let Some(sh) = cx.draw_shaders.shaders.get_mut(shader) {
                if let Some(dst) = sh.mapping.scope_uniforms_buf.get_mut(at..at + 4) {
                    dst.copy_from_slice(&v);
                    sh.mapping.scope_uniforms_gen = uniforms_gen;
                }
            }
        }
        PulseSlot::Clear { pass } => {
            let p = &mut cx.passes[pass];
            p.clear_color = Vec4f { x: v[0], y: v[1], z: v[2], w: v[3] };
            if p.main_draw_list_id.is_some() {
                p.paint_dirty = true;
            }
        }
    }
}

/// Bring every draw buffer in line with the pulse: re-tone the ledger's
/// slots (dropping any a widget has since written something else into),
/// then adopt every fresh occurrence of the true colour — a redraw writes
/// the true values back, and this runs again after each one.
fn pulse_sync(cx: &mut Cx, st: &mut PulseState) -> usize {
    let pulsed = st.fixed.unwrap_or_else(|| pulse_tone(st.target, st.m));
    let target = st.target;
    let last = st.last;
    let live: std::collections::HashSet<DrawListId> = cx.draw_lists.id_iter().collect();
    let passes: Vec<DrawPassId> = cx.passes.id_iter().collect();
    let mut ledger = std::mem::take(&mut st.ledger);
    ledger.retain(|s| {
        match *s {
            PulseSlot::Inst { list, .. } | PulseSlot::Uni { list, .. } if !live.contains(&list) => return false,
            PulseSlot::Clear { pass } if !passes.contains(&pass) => return false,
            _ => {}
        }
        match pulse_slot_get(cx, *s) {
            Some(v) if pulse_close(&v, target) || pulse_close(&v, last) => {
                pulse_slot_set(cx, *s, pulsed);
                true
            }
            _ => false,
        }
    });
    let known: std::collections::HashSet<PulseSlot> = ledger.iter().copied().collect();
    let mut shader_slots: std::collections::HashMap<usize, (usize, Vec<usize>, Vec<usize>)> = Default::default();
    for list_id in live {
        let item_count = cx.draw_lists[list_id].draw_items.len();
        for item_id in 0..item_count {
            let Some(shader_index) = cx.draw_lists[list_id].draw_items[item_id]
                .draw_call()
                .map(|dc| dc.draw_shader_id.index)
            else {
                continue;
            };
            let (stride, inst_offs, uni_offs) = shader_slots
                .entry(shader_index)
                .or_insert_with(|| {
                    let mapping = &cx.draw_shaders.shaders[shader_index].mapping;
                    (
                        mapping.instances.total_slots,
                        mapping.instances.inputs.iter().filter(|i| i.slots == 4).map(|i| i.offset).collect(),
                        mapping.dyn_uniforms.inputs.iter().filter(|i| i.slots == 4).map(|i| i.offset).collect(),
                    )
                })
                .clone();
            let mut fresh: Vec<PulseSlot> = Vec::new();
            {
                let item = &cx.draw_lists[list_id].draw_items[item_id];
                if let (Some(buf), true) = (item.instances.as_ref(), stride > 0) {
                    for n in 0..buf.len() / stride {
                        for off in &inst_offs {
                            let at = n * stride + off;
                            let s = PulseSlot::Inst { list: list_id, item: item_id, at };
                            if at + 4 <= buf.len() && !known.contains(&s) && pulse_close(&buf[at..at + 4], target) {
                                fresh.push(s);
                            }
                        }
                    }
                }
                if let Some(call) = item.draw_call() {
                    for off in &uni_offs {
                        let s = PulseSlot::Uni { list: list_id, item: item_id, at: *off };
                        if *off + 4 <= call.dyn_uniforms.len()
                            && !known.contains(&s)
                            && pulse_close(&call.dyn_uniforms[*off..*off + 4], target)
                        {
                            fresh.push(s);
                        }
                    }
                }
            }
            for s in fresh {
                pulse_slot_set(cx, s, pulsed);
                ledger.push(s);
            }
        }
    }
    // Theme colours referenced inside shader code, and pass clear colours.
    let mut fresh: Vec<PulseSlot> = Vec::new();
    for shader in 0..cx.draw_shaders.shaders.len() {
        let mapping = &cx.draw_shaders.shaders[shader].mapping;
        for input in mapping.scope_uniforms.inputs.iter().filter(|i| i.slots == 4) {
            let at = input.offset;
            let s = PulseSlot::Scope { shader, at };
            if let Some(b) = mapping.scope_uniforms_buf.get(at..at + 4) {
                if !known.contains(&s) && pulse_close(b, target) {
                    fresh.push(s);
                }
            }
        }
    }
    for pass in passes {
        let s = PulseSlot::Clear { pass };
        let p = &cx.passes[pass];
        // A pass without a draw list never paints: nothing to see, and
        // marking it dirty only logs "Draw pass has no draw list!".
        if p.main_draw_list_id.is_none() {
            continue;
        }
        let c = p.clear_color;
        if !known.contains(&s) && pulse_close(&[c.x, c.y, c.z, c.w], target) {
            fresh.push(s);
        }
    }
    for s in fresh {
        pulse_slot_set(cx, s, pulsed);
        ledger.push(s);
    }
    st.last = pulsed;
    st.ledger = ledger;
    st.ledger.len()
}

/// Repaint (no redraw) what the pulse touched: the window passes, the
/// passes of every patched draw list, and every patched clear colour.
fn pulse_repaint(cx: &mut Cx, st: &PulseState) {
    let mut dirty: Vec<DrawPassId> = Vec::new();
    for pass in cx.passes.id_iter() {
        let p = &cx.passes[pass];
        if matches!(p.parent, CxDrawPassParent::Window(_)) && p.main_draw_list_id.is_some() {
            dirty.push(pass);
        }
    }
    for s in &st.ledger {
        match *s {
            PulseSlot::Inst { list, .. } | PulseSlot::Uni { list, .. } => {
                if let Some(pass) = cx.draw_lists[list].draw_pass_id {
                    dirty.push(pass);
                }
            }
            PulseSlot::Clear { pass } => dirty.push(pass),
            PulseSlot::Scope { .. } => {}
        }
    }
    for pass in dirty {
        cx.passes[pass].paint_dirty = true;
    }
}

/// Write the true colour back into every slot still holding the pulse.
fn pulse_restore(cx: &mut Cx, st: &PulseState) {
    for s in &st.ledger {
        if let Some(v) = pulse_slot_get(cx, *s) {
            if pulse_close(&v, st.last) {
                pulse_slot_set(cx, *s, st.target);
            }
        }
    }
}

/// The post-draw hook while a pulse is live: widgets that just redrew
/// wrote true colours; re-apply before the paint.
fn pulse_after_draw(cx: &mut Cx) {
    theme_overrides_sync(cx);
    let st = session().lock().unwrap().pulse.take();
    if let Some(mut st) = st {
        pulse_sync(cx, &mut st);
        session().lock().unwrap().pulse = Some(st);
    }
}

/// Re-apply every theme colour edit to the draw buffers (widgets that
/// redrew wrote their baked colour back).
fn theme_overrides_sync(cx: &mut Cx) {
    let mut overrides = std::mem::take(&mut session().lock().unwrap().theme_overrides);
    for (_, st) in overrides.iter_mut() {
        pulse_sync(cx, st);
    }
    session().lock().unwrap().theme_overrides = overrides;
}

/// The post-draw hook is up exactly while a pulse or a theme edit lives.
fn hook_sync(cx: &mut Cx) {
    let live = {
        let s = session().lock().unwrap();
        s.pulse.is_some() || !s.theme_overrides.is_empty()
    };
    cx.post_draw_hook = if live { Some(Box::new(pulse_after_draw)) } else { None };
}

/// One global theme value: a colour (packed `0xrrggbbaa`) or a number.
/// Part of the [`crate::reflect`] surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThemeVal {
    Color(u32),
    Num(f64),
}

/// The app's theme: every colour and number in `mod.theme`, with the
/// level (file:line) that defines it. Read from the script cascade. Part of
/// the [`crate::reflect`] surface: a theme panel's rows and the probe behind
/// `/tweak/op?op=theme&name=`.
pub fn theme_values(cx: &mut Cx) -> Vec<(String, LiveId, ThemeVal, String)> {
    let mut out: Vec<(String, LiveId, ThemeVal, String)> = Vec::new();
    cx.with_vm(|vm| {
        let theme = vm.module(id!(theme));
        let chain = vm.construction_chain(theme.into());
        let mut keys: Vec<(LiveId, String)> = Vec::new();
        for lvl in &chain {
            let loc = lvl
                .loc
                .as_ref()
                .map(|l| format!("{}:{}", l.file.rsplit('/').next().unwrap_or(&l.file), l.line))
                .unwrap_or_default();
            for key in &lvl.own_keys {
                if !keys.iter().any(|(k, _)| k == key) {
                    keys.push((*key, loc.clone()));
                }
            }
        }
        for (key, loc) in keys {
            let name = live_id_token(key);
            let value = vm.bx.heap.value(theme, key.into(), NoTrap);
            if let Some(color) = value.as_color() {
                out.push((name, key, ThemeVal::Color(color), loc));
            } else if let Some(f) = value.as_f64() {
                out.push((name, key, ThemeVal::Num(f), loc));
            }
        }
    });
    out
}

/// The theme's colours (`color_*`), for the chips, the pulse and the strip.
fn theme_palette(cx: &mut Cx) -> Vec<(String, u32, String)> {
    theme_values(cx)
        .into_iter()
        .filter_map(|(name, _, value, loc)| match value {
            ThemeVal::Color(c) if name.starts_with("color") => Some((name, c, loc)),
            _ => None,
        })
        .collect()
}

/// Overwrite one theme value in the script heap, wherever in the theme's
/// prototype chain it is defined (the module is immutable to scripts).
fn theme_heap_set(cx: &mut Cx, key: LiveId, value: ScriptValue) -> bool {
    let mut hit = false;
    cx.with_vm(|vm| {
        let theme = vm.module(id!(theme));
        vm.proto_map_iter_mut_with(theme, &mut |_, map| {
            if let Some(entry) = map.get_mut(&key.into()) {
                entry.value = value;
                hit = true;
            }
        });
    });
    hit
}

/// One theme value as the text the ledger, the sidebar and the remote op
/// all show: `#rrggbbaa` for a colour, `fmt_f64` for a number.
pub(crate) fn theme_val_text(value: ThemeVal) -> String {
    match value {
        ThemeVal::Color(c) => hex_of(c),
        ThemeVal::Num(f) => fmt_f64(f),
    }
}

/// The text a theme edit applies. `theme.other` as a value means "what
/// `other` is right now", so a colour can be pointed at another token
/// without copying its hex; anything else is taken as written.
pub(crate) fn theme_edit_text(
    values: &[(String, LiveId, ThemeVal, String)],
    text: &str,
) -> Result<String, String> {
    match text.strip_prefix("theme.") {
        Some(other) => values
            .iter()
            .find(|(n, _, _, _)| n == other)
            .map(|(_, _, v, _)| theme_val_text(*v))
            .ok_or_else(|| format!("no theme value {other:?}")),
        None => Ok(text.to_string()),
    }
}

/// Where a theme edit's undo step goes: the session's own stack
/// ([`track_undo`]) for a fresh gesture, nothing for an undo/redo replay
/// that is re-applying a step already on it.
pub(crate) type UndoSink = fn(&mut TweakSession, &TweakDiffEntry);

/// The ONE way a theme value changes at runtime, shared by the overlay's
/// sidebar, its reset and undo/redo, the remote `op=theme` and
/// [`crate::reflect::theme_set_value`]. A colour is retargeted in every
/// draw-buffer slot that holds it through the pulse's identity ledger
/// (re-applied after each draw while the edit lives) and written into the
/// theme object in the script heap so later applies bake it too; a number
/// only lands in the heap (layouts re-flow when the edit reaches the
/// source). Every change is ledgered under the token's own definition site
/// with scope "theme" and, given a sink, pushed as an undo step.
///
/// The session is the process global behind [`session`]; it is not a
/// parameter because the pulse helpers this path calls
/// (`theme_overrides_sync`, `hook_sync`) address that same global, so
/// passing another lock in would only pretend to be injection.
///
/// Returns the value's kind before the edit and the text applied (with
/// `theme.x` aliases resolved), or `None` when the value already was what
/// was asked and nothing was touched.
pub(crate) fn theme_apply(
    cx: &mut Cx,
    name: &str,
    text: &str,
    origin: &str,
    undo: Option<UndoSink>,
) -> Result<Option<(ThemeVal, String)>, String> {
    let values = theme_values(cx);
    let (key, value, loc) = values
        .iter()
        .find(|(n, _, _, _)| n == name)
        .map(|(_, k, v, l)| (*k, *v, l.clone()))
        .ok_or_else(|| format!("no theme value {name:?}"))?;
    let text = theme_edit_text(&values, text)?;
    let old_text = theme_val_text(value);
    let overridden = session().lock().unwrap().theme_overrides.iter().any(|(n, _)| n == name);
    if old_text == text && !overridden {
        return Ok(None);
    }
    match value {
        ThemeVal::Color(current) => {
            let (rgba, _) = parse_hex(&text).ok_or_else(|| format!("{text:?} is not a colour"))?;
            let new = packed_of(rgba);
            // The theme module is immutable to scripts; a design tool
            // edits the value in place, at the level that defines it.
            theme_heap_set(cx, key, ScriptValue::from_color(new));
            let mut s = session().lock().unwrap();
            match s.theme_overrides.iter().position(|(n, _)| n == name) {
                Some(i) => {
                    if new == packed_of(s.theme_overrides[i].1.target) {
                        // Back at the original: restore and forget.
                        let (_, st) = s.theme_overrides.remove(i);
                        drop(s);
                        pulse_restore(cx, &st);
                    } else {
                        s.theme_overrides[i].1.fixed = Some(rgba);
                        drop(s);
                    }
                }
                None => {
                    let mut st = PulseState::new(current);
                    st.fixed = Some(rgba);
                    s.theme_overrides.push((name.to_string(), st));
                    drop(s);
                }
            }
            theme_overrides_sync(cx);
            hook_sync(cx);
            let overrides = std::mem::take(&mut session().lock().unwrap().theme_overrides);
            for (_, st) in &overrides {
                pulse_repaint(cx, st);
            }
            session().lock().unwrap().theme_overrides = overrides;
        }
        ThemeVal::Num(_) => {
            let f: f64 = text.parse().map_err(|_| format!("{text:?} is not a number"))?;
            // Numbers are baked into layouts at apply time: the heap
            // holds the new value for everything applied from now on
            // and the ledger carries it to the source; existing layout
            // re-flows when the edit lands (a live reload cannot
            // redefine the immutable widget modules today).
            theme_heap_set(cx, key, ScriptValue::from_f64(f));
        }
    }
    let now = cx.seconds_since_app_start();
    let mut s = session().lock().unwrap();
    s.suppress_until = now + SUPPRESS_LINGER;
    s.apply_gen += 1;
    s.next_seq += 1;
    let entry = TweakDiffEntry {
        seq: s.next_seq,
        path: "theme".to_string(),
        prop: name.to_string(),
        old: old_text,
        new: text.clone(),
        origin: loc,
        siblings: 0,
        scope: "theme".to_string(),
    };
    if let Some(track) = undo {
        log!("TWEAK {} theme {} {} -> {} ({})", origin, entry.prop, entry.old, entry.new, entry.origin);
        track(&mut s, &entry);
    }
    s.diff.push(entry);
    drop(s);
    cx.redraw_all();
    Ok(Some((value, text)))
}

fn rgba_of(c: u32) -> [f32; 4] {
    [
        ((c >> 24) & 0xff) as f32 / 255.0,
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
    ]
}

fn packed_of(rgba: [f32; 4]) -> u32 {
    ((rgba[0] * 255.0).round() as u32) << 24
        | ((rgba[1] * 255.0).round() as u32) << 16
        | ((rgba[2] * 255.0).round() as u32) << 8
        | ((rgba[3] * 255.0).round() as u32)
}

/// A measured length for the eye: one decimal at most, and no trailing `.0`.
/// `fmt_f64` keeps four, which turns a Fill width into `420.7333` and pushes
/// the readout past the panel's edge.
fn fmt_measure(v: f64) -> String {
    let rounded = (v * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < f64::EPSILON {
        format!("{}", rounded.round() as i64)
    } else {
        format!("{rounded:.1}")
    }
}

fn hex_of(c: u32) -> String {
    format_hex(rgba_of(c), true)
}

/// The theme rows' place in `rows_uid`: no widget, the theme itself.
const THEME_ROWS: u64 = u64::MAX;

/// Selected state by fill, never by brackets in the label.
///
/// ALL FIVE fills, not just the resting one. A Button's face is
/// `color`/`color_hover`/`color_down`/`color_focus` mixed by the animator,
/// and it takes key focus on click — so setting `color` alone left every
/// toggle you had just clicked painting the theme's focus grey instead of
/// its own state, indefinitely. The state was correct and invisible: the
/// last button touched always looked the same whichever way it was set,
/// which is what made the whole row unreadable. The gradient end stops go
/// flat (negative alpha) for the same reason — the theme gives the hover and
/// focus states a second stop, and a two-tone face reads as a third state
/// that does not exist.
fn set_button_fill(cx: &mut Cx, btn: WidgetRef, selected: bool) {
    let mut btn = btn;
    let (base, hover, down): (Vec4f, Vec4f, Vec4f) = if selected {
        (
            vec4(0.23, 0.45, 0.83, 1.0),
            vec4(0.31, 0.54, 0.92, 1.0),
            vec4(0.17, 0.35, 0.67, 1.0),
        )
    } else {
        (
            vec4(0.17, 0.17, 0.18, 1.0),
            vec4(0.27, 0.27, 0.29, 1.0),
            vec4(0.12, 0.12, 0.13, 1.0),
        )
    };
    let flat: Vec4f = vec4(-1.0, -1.0, -1.0, -1.0);
    script_apply_eval!(cx, btn, {
        draw_bg +: {
            color: #(base)
            color_hover: #(hover)
            color_down: #(down)
            color_focus: #(base)
            color_2: #(flat)
            color_2_hover: #(flat)
            color_2_down: #(flat)
            color_2_focus: #(flat)
        }
    });
}

/// Keep the leaf visible: `…` then the last `keep` chars.
fn tail_ellipsis(s: &str, keep: usize) -> String {
    let n = s.chars().count();
    if n <= keep {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - keep).collect();
    format!("\u{2026}{tail}")
}

/// Which kind of place a cascade level comes from, for its icon:
/// 0 app file, 1 widget library file, 2 theme file, 3 native (no file).
fn cascade_icon_kind(file: &str) -> usize {
    if file.is_empty() {
        3
    } else if file.rsplit('/').next().unwrap_or("").contains("theme") {
        2
    } else if file.contains("widgets/src/") || file.contains("/widgets/") {
        1
    } else {
        0
    }
}

/// A history step's widget: the path when it round-trips, else the pinned
/// selection when the step was recorded against it (paths through
/// anonymous segments — `-`, list indices — never round-trip the finder,
/// and every undo of a scrub on such a widget silently did nothing).
fn resolve_widget_for_history(cx: &mut Cx, path: &str) -> Result<WidgetRef, String> {
    if let Ok(w) = resolve_widget_by_path(cx, path) {
        return Ok(w);
    }
    let pinned = session().lock().unwrap().pinned.clone();
    if let Some(p) = pinned {
        if p.path == path {
            let w = cx.widget_tree().widget(WidgetUid(p.uid));
            if !w.is_empty() {
                return Ok(w);
            }
        }
    }
    Err(format!("no widget at {path}"))
}

/// The compiled shader a widget's draw layer is drawing with, read
/// through the layer's live draw call (the primary material through the
/// widget's own area).
fn layer_shader_id(cx: &Cx, widget: &WidgetRef, layer: &str, primary: bool) -> Option<DrawShaderId> {
    let area = widget
        .layer_areas()
        .into_iter()
        .find(|(name, _)| *name == layer)
        .map(|(_, a)| a)
        .or_else(|| if primary { Some(widget.area()) } else { None })?;
    let Area::Instance(inst) = area else { return None };
    if inst.instance_count == 0 {
        return None;
    }
    let draw_list = &cx.draw_lists[inst.draw_list_id];
    if inst.draw_item_id >= draw_list.draw_items.len() {
        return None;
    }
    Some(draw_list.draw_items[inst.draw_item_id].draw_call()?.draw_shader_id)
}

/// The hot-patchable constants of one draw layer, as (index, name, doc,
/// initial, value, loc) — copied out so the caller may use cx freely.
fn layer_consts(cx: &mut Cx, widget: &WidgetRef, layer: &str, primary: bool) -> Vec<(DrawShaderId, usize, String, String, f32, f32, String)> {
    let mut out = Vec::new();
    let Some(shader) = layer_shader_id(cx, widget, layer, primary) else { return out };
    let raw: Vec<(usize, String, String, f32, f32, ScriptIp)> = cx
        .shader_const_table(shader)
        .iter()
        .enumerate()
        .map(|(i, tc)| (i, tc.name.clone(), tc.doc.clone(), tc.initial, tc.value, tc.ip))
        .collect();
    for (i, name, doc, initial, value, ip) in raw {
        // ip_to_loc names the literal's exact line (711a490ce fixed the
        // tokenizer's float-column measurement and script_mod!'s row 0).
        let loc = cx.with_vm(|vm| vm.bx.code.ip_to_loc(ip)).map(|l| {
            let base = l.file.rsplit('/').next().unwrap_or(&l.file).to_string();
            format!("{base}:{}", l.line)
        });
        out.push((shader, i, name, doc, initial, value, loc.unwrap_or_else(|| "?".to_string())));
    }
    out
}

/// A shader constant of the widget by name, across its draw layers.
fn const_lookup(cx: &mut Cx, widget: &WidgetRef, name: &str) -> Option<(DrawShaderId, usize, String, f32, f32)> {
    let mut layers: Vec<(String, bool)> = widget.layer_areas().into_iter().map(|(n, _)| (n.to_string(), false)).collect();
    if layers.is_empty() {
        layers.push(("draw_bg".to_string(), true));
    } else {
        layers[0].1 = true;
    }
    for (layer, primary) in layers {
        for (shader, i, cname, _doc, initial, value, loc) in layer_consts(cx, widget, &layer, primary) {
            if cname == name {
                return Some((shader, i, loc, initial, value));
            }
        }
    }
    None
}

/// Set (Some) or reset (None) a shader constant by name: the GPU takes it
/// next frame, no recompile, source untouched. Ledgered with the literal's
/// file:line and scope "shader" — every draw sharing that compiled shader
/// changes with it. Returns (old, new).
fn const_set(cx: &mut Cx, widget: &WidgetRef, path: &str, name: &str, value: Option<f64>, origin: &str) -> Result<(f32, f32), String> {
    let (shader, index, loc, initial, old) = const_lookup(cx, widget, name).ok_or_else(|| format!("no shader constant named {name:?} on this widget"))?;
    // Every site in the shader annotated with this name is the same knob.
    let sites: Vec<usize> = cx
        .shader_const_table(shader)
        .iter()
        .enumerate()
        .filter(|(_, tc)| tc.name == name)
        .map(|(i, _)| i)
        .collect();
    let sites = if sites.is_empty() { vec![index] } else { sites };
    // A settle that lands on the value already on the GPU (the Ended after
    // a scrub, a typed value equal to the current) is not an edit.
    let target = match value {
        Some(v) => v as f32,
        None => initial,
    };
    if target == old {
        return Ok((old, old));
    }
    let new = match value {
        Some(v) => {
            for i in &sites {
                if !cx.shader_const_patch(shader, *i, v as f32) {
                    return Err(format!("shader constant {name:?} could not be patched"));
                }
            }
            v as f32
        }
        None => {
            for i in &sites {
                cx.shader_const_reset(shader, *i);
            }
            initial
        }
    };
    let now = cx.seconds_since_app_start();
    let mut s = session().lock().unwrap();
    s.suppress_until = now + SUPPRESS_LINGER;
    s.apply_gen += 1;
    s.next_seq += 1;
    let entry = TweakDiffEntry {
        seq: s.next_seq,
        path: path.to_string(),
        prop: format!("const:{name}"),
        old: fmt_f64(old as f64),
        new: fmt_f64(new as f64),
        origin: format!("{loc} \u{00b7} shader {}: every widget drawing with it", shader.index),
        siblings: 0,
        scope: "shader".to_string(),
    };
    if origin != "undo" && origin != "redo" {
        log!("TWEAK {} {} {} {} -> {} ({})", origin, entry.path, entry.prop, entry.old, entry.new, entry.origin);
        track_undo(&mut s, &entry);
    }
    s.diff.push(entry);
    drop(s);
    cx.redraw_all();
    Ok((old, new))
}

/// How many component fields a typed editor has (the position of a field
/// uid modulo this is its component, whichever copy of the row it sits in).
fn comp_count(kind: StructKind) -> usize {
    match kind {
        StructKind::Vec2 => 2,
        StructKind::Vec3 => 3,
        StructKind::Vec4 | StructKind::Inset => 4,
        StructKind::Metrics => 3,
        _ => 1,
    }
}

/// The number token after `key:` in a `{..}` dump (`null` reads as 0).
fn struct_num(text: &str, key: &str) -> Option<f64> {
    let at = text.find(&format!("{key}:"))?;
    let rest = text[at + key.len() + 1..].trim_start();
    let tok: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '}' && *c != ',')
        .collect();
    if tok == "null" {
        return Some(0.0);
    }
    tok.parse().ok()
}

/// Classify a reflected value dump into a typed editor + its component
/// values (empty for NoEditor). Only dumps reach here — scalars, colours
/// and bools are already their own row kinds.
fn parse_struct(value: &str) -> (StructKind, Vec<f64>) {
    let v = value.trim();
    for (pfx, k, n) in [
        ("vec2f(", StructKind::Vec2, 2usize),
        ("vec3f(", StructKind::Vec3, 3),
        ("vec4f(", StructKind::Vec4, 4),
    ] {
        if let Some(inner) = v.strip_prefix(pfx).and_then(|s| s.strip_suffix(')')) {
            let nums: Vec<f64> = inner.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            if nums.len() == n {
                return (k, nums);
            }
        }
    }
    if v.starts_with('{') && v.ends_with('}') {
        if v.contains("left:") && v.contains("bottom:") {
            return (
                StructKind::Inset,
                vec![
                    struct_num(v, "left").unwrap_or(0.0),
                    struct_num(v, "top").unwrap_or(0.0),
                    struct_num(v, "right").unwrap_or(0.0),
                    struct_num(v, "bottom").unwrap_or(0.0),
                ],
            );
        }
        if v.contains("descender:") {
            return (
                StructKind::Metrics,
                vec![
                    struct_num(v, "descender").unwrap_or(0.0),
                    struct_num(v, "line_gap").unwrap_or(0.0),
                    struct_num(v, "line_scale").unwrap_or(1.0),
                ],
            );
        }
        return (StructKind::NoEditor, Vec::new());
    }
    (StructKind::None, Vec::new())
}

/// Every `/** */` annotation behind a widget's properties, gathered up its
/// construction chain: property name (dotted for a sub-object's field, as
/// [`reflect_flat`] names it) to the doc's text. Part of the
/// [`crate::reflect`] surface: a Docs panel's third column.
pub fn collect_row_docs(cx: &mut Cx, widget: &WidgetRef) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return out;
    }
    cx.with_vm(|vm| {
        let chain = vm.construction_chain(source.into());
        for lvl in &chain {
            for (f, d) in &lvl.field_docs {
                out.entry(live_id_token(*f)).or_insert_with(|| d.clone());
            }
        }
        for lvl in &chain {
            for key in &lvl.own_keys {
                let value = vm.bx.heap.value(lvl.object, (*key).into(), NoTrap);
                let Some(obj) = value.as_object() else { continue };
                if vm.bx.heap.as_fn(obj).is_some() {
                    continue;
                }
                let name = live_id_token(*key);
                for sub in vm.construction_chain(value) {
                    if let Some(doc) = &sub.doc {
                        out.entry(name.clone()).or_insert_with(|| doc.clone());
                    }
                    for (f, d) in &sub.field_docs {
                        out.entry(format!("{name}.{}", live_id_token(*f)))
                            .or_insert_with(|| d.clone());
                    }
                }
            }
        }
    });
    out
}

fn cascade_levels(cx: &mut Cx, widget: &WidgetRef) -> Vec<CascadeLevel> {
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return Vec::new();
    }
    cx.with_vm(|vm| {
        let chain = vm.construction_chain(source.into());
        let mut seen: std::collections::HashSet<String> = Default::default();
        let mut out = Vec::new();
        for lvl in chain {
            let (loc, file, line) = match &lvl.loc {
                Some(l) => {
                    let base = l.file.rsplit('/').next().unwrap_or(&l.file);
                    (format!("{base}:{}", l.line), l.file.clone(), l.line)
                }
                None => ("native".to_string(), String::new(), 0),
            };
            let mut sets: Vec<(String, bool)> = Vec::new();
            for key in &lvl.own_keys {
                let name = live_id_token(*key);
                if name.starts_with("__") || skip_key(&name) {
                    continue;
                }
                let overridden = seen.contains(&name);
                sets.push((name, overridden));
            }
            for (name, _) in &sets {
                seen.insert(name.clone());
            }
            out.push(CascadeLevel {
                loc,
                file,
                line,
                doc: lvl.doc.clone(),
                field_docs: lvl
                    .field_docs
                    .iter()
                    .map(|(f, d)| (live_id_token(*f), d.clone()))
                    .collect(),
                sets,
            });
        }
        out
    })
}

/// Every widget's path, in names a person can read.
///
/// `WidgetTree::path_to` renders an unnamed node as `-` and a list item as a
/// bare index, so a real path came out as `-0.main_window.body.dock.-.-.3` —
/// the same string for every unnamed sibling, and no help to anyone reading
/// it. Here each segment is, in order of preference:
///
/// * the node's own name (`dock`, `tOverview`, `press_demo`), or
/// * its TYPE when it has no name of its own (`View`, `Label`), and
/// * `.1`, `.2`… appended when siblings would otherwise collide — four
///   unnamed Labels under one View become `Label.1`..`Label.4`. The slash is
///   the hierarchy; a dot is only which one of several.
///
/// The head is dropped, because it is on every path in the app and therefore
/// tells nobody anything: the tree root (which has neither a name nor a
/// reliably-registered type), any single-child chain under it, and the
/// `Window`'s own `body` container. What is left starts at the first thing
/// the app itself put on screen — `dock.tOverview.View.View.Label_1`.
///
/// Built from `flat_tree`, whose depth-first order and depth column give both
/// the parent chain and the sibling order. That is a whole-tree walk, so this
/// is for CLICKS (copy, @mention, a selection change) — never for hover.
fn readable_paths(cx: &Cx) -> Vec<(u64, String)> {
    let rows = cx.widget_tree().flat_tree(cx);
    let n = rows.len();
    // The parent of each row, read straight off the depth-first order.
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut stack: Vec<usize> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        stack.truncate(row.depth as usize);
        parent[i] = stack.last().copied();
        stack.push(i);
    }
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            children[*p].push(i);
        }
    }
    let mut segment: Vec<String> = rows
        .iter()
        .map(|row| readable_segment(&row.name, &row.ty))
        .collect();
    // Siblings that want the same segment get numbered, in child order.
    let roots: Vec<usize> = (0..n).filter(|i| parent[*i].is_none()).collect();
    let sibling_groups = children
        .iter()
        .cloned()
        .chain(std::iter::once(roots.clone()))
        .filter(|group| group.len() > 1);
    for group in sibling_groups {
        let clashing: Vec<usize> = group
            .iter()
            .copied()
            .filter(|i| group.iter().filter(|j| segment[**j] == segment[*i]).count() > 1)
            .collect();
        let mut ordinal: HashMap<String, usize> = HashMap::new();
        for i in clashing {
            let base = segment[i].clone();
            let next = ordinal.entry(base.clone()).or_insert(0);
            *next += 1;
            segment[i] = format!("{base}.{next}");
        }
    }
    // How many leading segments are the shared, meaningless head: the root
    // itself, then any node that is the only way down, then the Window's
    // `body`. Never so many that a path runs out.
    let mut skip = 1usize; // the root
    if roots.len() == 1 {
        let mut cur = roots[0];
        while children[cur].len() == 1 {
            let next = children[cur][0];
            if children[next].is_empty() {
                break; // the chain ends here: this node IS the path
            }
            cur = next;
            skip += 1;
        }
        // The Window's content container. It is named by the framework, it
        // wraps everything an app draws, and it is on every path.
        for child in children[cur].iter() {
            if segment[*child] == "body" && !children[*child].is_empty() {
                skip += 1;
                break;
            }
        }
    }
    let mut full: Vec<String> = Vec::with_capacity(n);
    for i in 0..n {
        let depth = rows[i].depth as usize;
        let path = match parent[i] {
            Some(_) if depth <= skip => format!("/{}", segment[i]),
            Some(p) => format!("{}/{}", full[p], segment[i]),
            None => format!("/{}", segment[i]),
        };
        full.push(path);
    }
    rows.iter().map(|row| row.uid).zip(full).collect()
}

/// The name the widget tree calls a widget, or empty when it has none worth
/// the word: `-` for anonymous, and a bare index for a list item.
fn tree_name_of(cx: &Cx, uid: u64) -> String {
    cx.widget_tree()
        .name_of(WidgetUid(uid))
        .map(live_id_token)
        .filter(|name| name != "-" && !name.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or_default()
}

/// One path segment, as a person would say it: the node's name, else its
/// type, else a last-resort placeholder. A name that is only digits (a list
/// item's index) is no name at all, so those take the type too.
fn readable_segment(name: &str, ty: &str) -> String {
    let named =
        !name.is_empty() && name != "-" && !name.bytes().all(|b| b.is_ascii_digit());
    if named {
        return name.to_string();
    }
    if !ty.is_empty() && ty != "-" {
        return ty.to_string();
    }
    "Widget".to_string()
}

/// One widget's readable path (`/window/body/Button.2`). See
/// [`readable_paths`]. Part of the [`crate::reflect`] surface: how an action
/// log names its sender.
pub fn indexed_path(cx: &Cx, uid: u64) -> String {
    readable_paths(cx)
        .into_iter()
        .find(|(u, _)| *u == uid)
        .map(|(_, path)| path)
        .unwrap_or_else(|| format!("uid:{uid}"))
}

/// The widget a path names. An exact match wins; otherwise anything whose
/// path ENDS with it, which is what makes a shortened, hand-written or
/// previously-stored tail still resolve.
fn resolve_indexed(cx: &Cx, path: &str) -> Option<WidgetUid> {
    let path = path.trim();
    if path.is_empty() || path == "/" {
        return None;
    }
    // A reference written or pasted without its leading slash still means
    // the same thing. Dots are NOT separators here — `Label.2` is one
    // segment, the second Label — so nothing else is normalised.
    let wanted = format!("/{}", path.trim_start_matches('/'));
    let paths = readable_paths(cx);
    if let Some((uid, _)) = paths.iter().find(|(_, full)| **full == wanted) {
        return Some(WidgetUid(*uid));
    }
    paths
        .iter()
        .find(|(_, full)| full.ends_with(&wanted))
        .map(|(uid, _)| WidgetUid(*uid))
}

/// Is this segment an anonymous node's numbered stand-in (`-`, `-2`)? Those
/// are positions, not names, so the loose finder must not turn them into ids.
fn is_anonymous_segment(segment: &str) -> bool {
    segment == "-"
        || (segment.starts_with('-') && segment.len() > 1 && segment[1..].bytes().all(|b| b.is_ascii_digit()))
}

/// The widget a readable path names: an indexed path (`/window/body/Button.2`)
/// taken as written, else a waypoint search over the named segments. Part
/// of the [`crate::reflect`] surface: how a story names its subject.
pub fn resolve_widget_by_path(cx: &Cx, path: &str) -> Result<WidgetRef, String> {
    let tree = cx.widget_tree();
    // An indexed path names one widget and nothing else: take it as written
    // before falling back to the waypoint search below, which drops the
    // anonymous segments and can only land on the nearest named ancestor.
    if let Some(uid) = resolve_indexed(cx, path) {
        let found = tree.widget(uid);
        if !found.is_empty() {
            return Ok(found);
        }
    }
    let ids: Vec<LiveId> = path
        .split(['.', '/'])
        .filter(|segment| !segment.is_empty() && !is_anonymous_segment(segment))
        .map(LiveId::from_str)
        .collect();
    if ids.is_empty() {
        return Err("empty path".to_string());
    }
    let root = tree.root_uid();
    let found = tree.find_within(root, &ids);
    if !found.is_empty() {
        return Ok(found);
    }
    // The full dotted path from /tweak/state includes ancestors the finder
    // treats as waypoints; a stale head (window renamed) can still resolve
    // from the tail.
    if ids.len() > 1 {
        let found = tree.find_within(root, &ids[1..]);
        if !found.is_empty() {
            return Ok(found);
        }
        let tail = [*ids.last().unwrap()];
        let found = tree.find_within(root, &tail);
        if !found.is_empty() {
            return Ok(found);
        }
    }
    Err(format!("no widget at path {path:?}"))
}

/// `file:line` of the site that constructed the widget's source object —
/// the first level of its construction chain. A template instance answers
/// with the template's `:=` literal; a one-off widget with its own literal.
fn source_origin(cx: &mut Cx, widget: &WidgetRef) -> String {
    cascade_levels(cx, widget)
        .first()
        .filter(|l| !l.file.is_empty())
        .map(|l| format!("{}:{}", l.file, l.line))
        .unwrap_or_default()
}

/// The type's definition site: the deepest cascade level that has a file
/// location — `mod.widgets.Button = set_type_default() do …` in the widget
/// library — where an "all Buttons" edit belongs in source.
fn type_origin(cx: &mut Cx, widget: &WidgetRef) -> String {
    cascade_levels(cx, widget)
        .iter()
        .rev()
        .find(|l| !l.file.is_empty())
        .map(|l| format!("{}:{}", l.file, l.line))
        .unwrap_or_else(|| source_origin(cx, widget))
}

/// Every other live widget of the same widget type — "every Button in the
/// system".
///
/// `confine` narrows that to one subtree (0 = the whole app). Isolation is
/// what sets it: with one branch on screen and the rest covered, "all
/// Buttons" plainly means the ones you can see, and an edit that also
/// reached the app you deliberately hid would be a surprise.
fn type_siblings(cx: &mut Cx, widget: &WidgetRef, confine: u64) -> Vec<WidgetRef> {
    let Some(ty) = widget.widget_type_id() else {
        return Vec::new();
    };
    let me = widget.widget_uid();
    let rows = cx.widget_tree().flat_tree(cx);
    let mut out = Vec::new();
    for row in rows {
        if row.uid == me.0 {
            continue;
        }
        let other = cx.widget_tree().widget(WidgetUid(row.uid));
        if other.is_empty() || other.widget_type_id() != Some(ty) {
            continue;
        }
        // The panel's own buttons/labels are the same types as the app's;
        // "all Button" means the app's buttons.
        let in_tweaker = cx
            .widget_tree()
            .path_to(WidgetUid(row.uid))
            .iter()
            .any(|id| *id == live_id!(tweaker));
        if in_tweaker {
            continue;
        }
        if confine != 0 && row.uid != confine && !is_ancestor_of(cx, confine, row.uid) {
            continue;
        }
        out.push(other);
    }
    out
}

/// Every other live widget built from the same source object (the same
/// template): the tabs beside a tab, the rows beside a list row.
fn template_siblings(cx: &mut Cx, widget: &WidgetRef) -> Vec<WidgetRef> {
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return Vec::new();
    }
    let me = widget.widget_uid();
    let rows = cx.widget_tree().flat_tree(cx);
    let mut out = Vec::new();
    for row in rows {
        if row.uid == me.0 {
            continue;
        }
        let other = cx.widget_tree().widget(WidgetUid(row.uid));
        if other.is_empty() || other.script_source() != source {
            continue;
        }
        out.push(other);
    }
    out
}

/// Values survive an apply (they live in the Rust draw vars) but the
/// SHADER is recomputed from the chain of the object just applied — and
/// every chunk derives a fresh object from the widget's untouched source,
/// so `draw_bg +: {color: #00f}` after a live `pixel: fn` edit compiled the
/// file's pixel back in (the fn edit vanished on the next colour tweak).
/// After a chunk that touches a layer with live fn edits, put those fns
/// back on top; their text is unchanged, so the compiled shader is a cache
/// hit.
fn reinject_fn_overrides(cx: &mut Cx, widget: &WidgetRef, chunk: &str) {
    let uid = widget.widget_uid().0;
    let overrides: Vec<((u64, String, String), String)> = session()
        .lock()
        .unwrap()
        .fn_overrides
        .iter()
        .filter(|((owner, _, _), _)| *owner == uid)
        .map(|(key, text)| (key.clone(), text.clone()))
        .collect();
    if overrides.is_empty() {
        return;
    }
    let mut layers: Vec<String> = Vec::new();
    for ((_, layer, name), _) in &overrides {
        let touches_layer = chunk.contains(&format!("{layer} +:"))
            || chunk.contains(&format!("{layer}:"))
            || chunk.contains(&format!("{layer}."));
        let defines_fn = chunk.contains(&format!("{name}:"));
        if touches_layer && !defines_fn && !layers.contains(layer) {
            layers.push(layer.clone());
        }
    }
    for layer in layers {
        let mut body = String::new();
        for ((_, l, _), text) in &overrides {
            if *l == layer {
                body.push_str(text);
                body.push('\n');
            }
        }
        let _ = makepad_platform::shader_error::take();
        if let Err(error) = eval_chunk(cx, widget, &format!("{layer} +: {{\n{body}}}")) {
            log!("TWEAK re-applying the live {layer} fns failed: {error}");
        }
        if let Some(error) = makepad_platform::shader_error::take() {
            log!("TWEAK re-applied live {layer} fns did not compile: {}", terse_shader_error(&error));
        }
    }
}

/// The first problem of a draw-shader compile report, without the
/// compiler's own source pointers; says how many more there were.
fn terse_shader_error(report: &str) -> String {
    let lines: Vec<&str> = report.lines().filter(|l| !l.trim().is_empty()).collect();
    let first = lines.first().copied().unwrap_or(report);
    // `DrawQuad: shader field …` — the type name is the layer's, keep it.
    let first = match first.find(" (platform/") {
        Some(at) => &first[..at],
        None => first,
    };
    let first: String = first.chars().take(160).collect();
    if lines.len() > 1 {
        format!("{first} (+{} more)", lines.len() - 1)
    } else {
        first
    }
}

/// Undo a chunk whose shader failed to compile: an fn chunk puts the
/// layer's fns back (the last text applied from the view or the AI, else
/// the fn as written); a single-property chunk puts the property back.
fn revert_failed_chunk(cx: &mut Cx, widget: &WidgetRef, chunk: &str, before: &[(String, String, bool)]) {
    if chunk.contains("fn") {
        if let Some(open) = chunk.find("+:") {
            let layer = chunk[..open].trim().to_string();
            revert_layer_fns(cx, widget, &layer);
            return;
        }
    }
    if let Some((prop, _)) = single_prop_chunk(chunk) {
        if let Some((_, old, quoted)) = before.iter().find(|(name, _, _)| *name == prop) {
            let text = if *quoted { format!("{old:?}") } else { old.clone() };
            let _ = makepad_platform::shader_error::take();
            if let Err(error) = eval_chunk(cx, widget, &format!("{prop}: {text}")) {
                log!("TWEAK revert of {prop} failed: {error}");
            }
            let _ = makepad_platform::shader_error::take();
        }
    }
}

fn revert_layer_fns(cx: &mut Cx, widget: &WidgetRef, layer: &str) {
    let uid = widget.widget_uid().0;
    let sources = layer_fn_sources(cx, widget, layer);
    let overrides = session().lock().unwrap().fn_overrides.clone();
    let mut body = String::new();
    for (name, _loc, src) in &sources {
        let text = overrides
            .get(&(uid, layer.to_string(), name.clone()))
            .cloned()
            .unwrap_or_else(|| src.clone());
        body.push_str(&text);
        body.push('\n');
    }
    if body.trim().is_empty() {
        return;
    }
    let _ = makepad_platform::shader_error::take();
    if let Err(error) = eval_chunk(cx, widget, &format!("{layer} +: {{\n{body}}}")) {
        log!("TWEAK revert of {layer} fns failed: {error}");
    }
    if let Some(error) = makepad_platform::shader_error::take() {
        log!("TWEAK revert of {layer} fns did not compile either: {}", terse_shader_error(&error));
    }
}

/// Rewrite every `tweak://apply:LINE:COL` in an error into the chunk's own
/// line numbers.
fn relocate_chunk_error(error: &str, callsite: u32) -> String {
    let mut out = String::new();
    let mut rest = error;
    while let Some(at) = rest.find("tweak://apply:") {
        out.push_str(&rest[..at]);
        let tail = &rest[at + "tweak://apply:".len()..];
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        match digits.parse::<u32>() {
            Ok(line) if line > callsite => {
                // The parser counts the `use` line and one more for the
                // wrapper: measured, `@@` on chunk line 4 reports callsite+5.
                out.push_str(&format!("line {}", line - callsite - 1));
                rest = &tail[digits.len()..];
            }
            _ => {
                out.push_str("tweak://apply:");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A stable pseudo-line for a chunk: FNV-1a of its text, so each distinct
/// chunk is its own script body (and its own fn ScriptIps).
fn chunk_callsite_line(code: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in code.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    (h % 1_000_000) + 2
}

/// The bare eval-apply: evaluate a splash chunk with the widget's own
/// `__script_source__` scope and apply it through the ordinary machinery.
/// No diff, no log — [`apply_splash_chunk`] wraps this for user-visible
/// edits; the tweaker's own scaffolding (body compression) uses it raw.
fn eval_chunk(cx: &mut Cx, widget: &WidgetRef, chunk: &str) -> Result<(), String> {
    let chunk = chunk.trim();
    let body = if chunk.starts_with('{') {
        chunk.to_string()
    } else {
        format!("{{{chunk}}}")
    };
    let code = format!("use mod.prelude.widgets.*\n__script_source__{body};");
    let callsite = chunk_callsite_line(&code);
    let errors = cx.with_vm(|vm| {
        // Install a captured-error sink so parse/apply problems come back to
        // the caller instead of only landing in the log.
        vm.bx.captured_errors = Some(Vec::new());
        let script_mod = ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: "tweak".to_string(),
            // The callsite is keyed by the chunk's own text: the draw-shader
            // cache hashes each fn's ScriptIp, and one fixed callsite reused
            // its body across applies — same ip, same hash, so the SECOND
            // `pixel: fn` rewrite on a widget was recorded everywhere and
            // never recompiled. Identical chunks still dedup to one body.
            file: "tweak://apply".to_string(),
            line: callsite as usize,
            column: 1,
            code,
            values: Vec::new(),
        };
        let mut target = widget.clone();
        use crate::makepad_script::traits::ScriptApply;
        target.script_apply_eval(vm, script_mod);
        vm.take_errors()
    });
    if !errors.is_empty() {
        // `tweak://apply:<callsite+n>:<col>` → `line <n>:<col>` of the chunk
        // (chunk line 1 sits on code line 2, right after the `use`).
        let errors: Vec<String> = errors
            .iter()
            .map(|e| relocate_chunk_error(e, callsite))
            .collect();
        return Err(format!("splash error: {}", errors.join("; ")));
    }
    Ok(())
}

/// Apply one splash chunk onto a widget: eval + ordinary apply + full
/// relayout; the value diff (flat reflection before vs after) is logged and
/// returned.
pub fn apply_splash_chunk(
    cx: &mut Cx,
    widget: &WidgetRef,
    path: &str,
    chunk: &str,
    origin: &str,
) -> Result<Vec<TweakDiffEntry>, String> {
    let chunk = chunk.trim();
    let before = reflect_flat(cx, widget);
    // The draw shader compiles inside the apply itself, so an fn that names
    // a field this layer does not have fails RIGHT HERE. Never answer ok
    // with a widget that stopped drawing: put the layer back and say why.
    let _ = makepad_platform::shader_error::take();
    eval_chunk(cx, widget, chunk)?;
    if let Some(error) = makepad_platform::shader_error::take() {
        revert_failed_chunk(cx, widget, chunk, &before);
        let terse = terse_shader_error(&error);
        log!("TWEAK {} {} rejected, shader did not compile: {}", origin, path, terse);
        return Err(format!("shader compile error: {terse}"));
    }
    // Template-built widgets (tabs, list items) share one source object;
    // an edit to one is an edit to the template, so every live sibling
    // takes it too — and the ledger names the template's site as the
    // place to change in source.
    // Scope decides who else takes the edit and which site the ledger
    // names: "this" = template siblings only (a tab is every tab) and the
    // instance's own site; "all" = every live widget of the TYPE and the
    // type's definition site.
    let (scope_all, isolate) = {
        let s = session().lock().unwrap();
        (s.scope_all, if s.scope_unconfined { 0 } else { s.isolate_uid })
    };
    // "all" confined to the isolated branch is a different promise from
    // "all" across the app, and it must not be recorded as the same thing:
    // the type's DEFINITION is the wrong place to write a change that was
    // deliberately kept to one branch, so the ledger names the branch's own
    // site and says which scope it was.
    let confined = scope_all && isolate != 0;
    let scope_name = if confined {
        "all in isolation"
    } else if scope_all {
        "all"
    } else {
        "this"
    };
    let origin_site = if confined {
        let root = cx.widget_tree().widget(WidgetUid(isolate));
        if root.is_empty() {
            type_origin(cx, widget)
        } else {
            source_origin(cx, &root)
        }
    } else if scope_all {
        type_origin(cx, widget)
    } else {
        source_origin(cx, widget)
    };
    let fan_out = if scope_all {
        type_siblings(cx, widget, isolate)
    } else {
        template_siblings(cx, widget)
    };
    reinject_fn_overrides(cx, widget, chunk);
    let mut siblings = 0u32;
    for sibling in fan_out {
        if eval_chunk(cx, &sibling, chunk).is_ok() {
            siblings += 1;
            reinject_fn_overrides(cx, &sibling, chunk);
        }
    }

    let after = reflect_flat(cx, widget);
    let now = cx.seconds_since_app_start();
    let mut changed = Vec::new();
    {
        let mut s = session().lock().unwrap();
        // The widget must be seen exactly as it renders while values move:
        // the solid outline yields to the faint stipple for a beat.
        s.suppress_until = now + SUPPRESS_LINGER;
        s.apply_gen += 1;
        // Sidebar/handle bursts (typing, scrubbing) log one line per pause,
        // not one per step; the diff records everything regardless.
        let interactive = origin != "remote";
        let should_log = |s: &mut TweakSession, prop: &str| -> bool {
            if !interactive {
                return true;
            }
            if let Some((last_path, last_prop, last_time)) = &s.last_sidebar_log {
                if last_path == path && last_prop == prop && now - *last_time < 1.0 {
                    return false;
                }
            }
            s.last_sidebar_log = Some((path.to_string(), prop.to_string(), now));
            true
        };
        // A `prop: theme.color_x` chunk ledgers the REFERENCE, not the hex
        // the reflection resolves it to — the AI writes the reference into
        // source.
        let theme_ref = single_prop_chunk(chunk)
            .filter(|(_, value)| value.trim().starts_with("theme."))
            .map(|(prop, value)| (prop, value.trim().to_string()));
        for (name, new_value, _) in &after {
            let old_value = before
                .iter()
                .find(|(old_name, _, _)| old_name == name)
                .map(|(_, value, _)| value.clone())
                .unwrap_or_else(|| "-".to_string());
            if &old_value != new_value {
                s.next_seq += 1;
                let shown_new = match &theme_ref {
                    Some((p, reference)) if p == name => reference.clone(),
                    _ => new_value.clone(),
                };
                let entry = TweakDiffEntry {
                    seq: s.next_seq,
                    path: path.to_string(),
                    prop: name.clone(),
                    old: old_value,
                    new: shown_new,
                    origin: origin_site.clone(),
                    siblings,
                    scope: scope_name.to_string(),
                };
                if should_log(&mut s, &entry.prop) {
                    log!(
                        "TWEAK {} {} {} {} -> {}",
                        origin,
                        entry.path,
                        entry.prop,
                        entry.old,
                        entry.new
                    );
                }
                s.diff.push(entry.clone());
                if origin != "undo" && origin != "redo" {
                    track_undo(&mut s, &entry);
                }
                changed.push(entry);
            }
        }
        if changed.is_empty() {
            // The chunk applied but the flat reflection saw no change. For a
            // dynamic shader input (border_radius: uniform(..)) the value
            // lives in the draw call, not a Rust field — a single-property
            // chunk still yields an honest (prop, old, new) entry: old is
            // the last applied value this session, else the reflected
            // default. Anything else logs as a raw chunk so nothing the
            // user did is silent.
            let entry = match single_prop_chunk(chunk) {
                Some((prop_name, value_text)) => {
                    let old = s
                        .diff
                        .iter()
                        .rev()
                        .find(|e| e.path == path && e.prop == prop_name)
                        .map(|e| e.new.clone())
                        .or_else(|| {
                            before
                                .iter()
                                .find(|(name, _, _)| *name == prop_name)
                                .map(|(_, value, _)| value.clone())
                        })
                        .unwrap_or_else(|| "-".to_string());
                    s.next_seq += 1;
                    TweakDiffEntry {
                        seq: s.next_seq,
                        path: path.to_string(),
                        prop: prop_name,
                        old,
                        new: value_text,
                        origin: origin_site.clone(),
                        siblings,
                        scope: scope_name.to_string(),
                    }
                }
                None => {
                    s.next_seq += 1;
                    TweakDiffEntry {
                        seq: s.next_seq,
                        path: path.to_string(),
                        prop: "(splash)".to_string(),
                        old: "-".to_string(),
                        new: chunk.to_string(),
                        origin: origin_site.clone(),
                        siblings,
                        scope: scope_name.to_string(),
                    }
                }
            };
            if should_log(&mut s, &entry.prop) {
                log!(
                    "TWEAK {} {} {} {} -> {}",
                    origin,
                    entry.path,
                    entry.prop,
                    entry.old,
                    entry.new
                );
            }
            s.diff.push(entry.clone());
            if origin != "undo" && origin != "redo" {
                track_undo(&mut s, &entry);
            }
            changed.push(entry);
        }
    }

    // Layout-affecting values (padding, margins, sizes) must take properly:
    // a FULL redraw re-runs every draw_walk, i.e. relayout, not just repaint.
    widget.redraw(cx);
    cx.redraw_all();
    Ok(changed)
}

// ---------------------------------------------------------------------------
// the remote surface — `Cx::tweak_callback`. Routes parse; this decides.
// ---------------------------------------------------------------------------

fn arg<'a>(args: &'a [(String, String)], keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some((_, value)) = args.iter().find(|(k, _)| k == key) {
            return Some(value.as_str());
        }
    }
    None
}

/// `"a.b: value"` (no braces, one property) — the shape every sidebar edit
/// and `prop=`/`value=` shorthand takes.
fn single_prop_chunk(chunk: &str) -> Option<(String, String)> {
    let chunk = chunk.trim();
    let inner = chunk.strip_prefix('{').map_or(chunk, |rest| rest.strip_suffix('}').unwrap_or(rest));
    let inner = inner.trim();
    if inner.contains('\n') || inner.contains('{') || inner.contains(',') {
        return None;
    }
    let (name, value) = inner.split_once(':')?;
    let name = name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    Some((name.to_string(), value.trim().to_string()))
}

fn fmt_f64(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1.0e15 {
        format!("{}", value as i64)
    } else {
        format!("{}", (value * 10000.0).round() / 10000.0)
    }
}

fn json_str(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for c in input.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn pick_json(pick: &TweakPick) -> String {
    format!(
        "{{\"path\":{},\"ty\":{},\"r\":[{:.0},{:.0},{:.0},{:.0}],\"w\":{}{}}}",
        json_str(&pick.path),
        json_str(&pick.ty),
        pick.rect.pos.x,
        pick.rect.pos.y,
        pick.rect.size.x,
        pick.rect.size.y,
        pick.window_id,
        match &pick.band {
            Some(band) => format!(",\"band\":{}", json_str(band)),
            None => String::new(),
        }
    )
}

fn diff_json(entries: &[TweakDiffEntry]) -> String {
    let mut out = String::from("[");
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"path\":{},\"prop\":{},\"old\":{},\"new\":{},\"origin\":{},\"siblings\":{},\"scope\":{}}}",
            json_str(&entry.path),
            json_str(&entry.prop),
            json_str(&entry.old),
            json_str(&entry.new),
            json_str(&entry.origin),
            entry.siblings,
            json_str(&entry.scope)
        ));
    }
    out.push(']');
    out
}

fn strokes_json(strokes: &[TweakStroke]) -> String {
    let mut out = String::from("[");
    for (index, stroke) in strokes.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!("{{\"w\":{},\"widgets\":[", stroke.window_id));
        for (windex, path) in stroke.widgets.iter().enumerate() {
            if windex > 0 {
                out.push(',');
            }
            out.push_str(&json_str(path));
        }
        out.push_str("],\"points\":[");
        for (pindex, (x, y)) in stroke.points.iter().enumerate() {
            if pindex > 0 {
                out.push(',');
            }
            out.push_str(&format!("[{x:.0},{y:.0}]"));
        }
        out.push_str("]}");
    }
    out.push(']');
    out
}

/// Coalesce the diff log: per (path, prop) the FIRST old and the LAST new;
/// churn collapsed, no-ops dropped.
fn coalesce_diff(entries: &[TweakDiffEntry]) -> Vec<TweakDiffEntry> {
    let mut out: Vec<TweakDiffEntry> = Vec::new();
    for entry in entries {
        match out
            .iter_mut()
            .find(|e| e.path == entry.path && e.prop == entry.prop)
        {
            Some(existing) => {
                existing.new = entry.new.clone();
                existing.seq = entry.seq;
            }
            None => out.push(entry.clone()),
        }
    }
    out.retain(|entry| entry.old != entry.new);
    out
}

/// The `Cx::tweak_callback` the widgets crate registers in `set_ui_root`.
pub fn tweak_callback(
    cx: &mut Cx,
    op: &str,
    args: &[(String, String)],
) -> Result<String, String> {
    match op {
        "toggle" => {
            let on = match arg(args, &["on"]) {
                Some(value) => !matches!(value, "0" | "false" | "off" | "no"),
                None => !tweak_is_on(),
            };
            if let Some(annotate) = arg(args, &["annotate", "draw"]) {
                session().lock().unwrap().annotate =
                    !matches!(annotate, "0" | "false" | "off" | "no");
            }
            set_tweak_on(cx, on);
            let annotate = session().lock().unwrap().annotate;
            Ok(format!(
                "{{\"on\":{},\"annotate\":{}}}",
                if on { 1 } else { 0 },
                if annotate { 1 } else { 0 }
            ))
        }
        // TEMP DEBUG RIG (do not commit): /tweak/op?op=perf — enable the
        // perf monitor and dump ring averages for framerate diagnosis.
        "perf" => {
            if !cx.perf_monitor.enabled() {
                cx.perf_monitor.set_enabled(true);
                return Ok("{\"enabled\":1,\"note\":\"call again for data\"}".to_string());
            }
            let mut frames = Vec::new();
            cx.perf_monitor.read(&mut frames);
            let live: Vec<_> = frames.iter().filter(|f| f.gap_ms > 0.0).collect();
            let n = live.len().max(1) as f32;
            let avg_gap: f32 = live.iter().map(|f| f.gap_ms).sum::<f32>() / n;
            let max_gap: f32 = live.iter().map(|f| f.gap_ms).fold(0.0, f32::max);
            let mut sorted: Vec<f32> = live.iter().map(|f| f.gap_ms).collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p95 = sorted.get(sorted.len().saturating_sub(1) * 95 / 100).copied().unwrap_or(0.0);
            let mut out = format!(
                "{{\"frames_painted\":{},\"ring_frames\":{},\"gap_avg_ms\":{:.2},\"gap_p95_ms\":{:.2},\"gap_max_ms\":{:.2},\"channels\":{{",
                cx.perf_monitor.frames_painted(), live.len(), avg_gap, p95, max_gap
            );
            let names: Vec<String> = cx.perf_monitor.channels().iter().map(|c| c.name.clone()).collect();
            for (i, name) in names.iter().enumerate() {
                if i > 0 { out.push(','); }
                let avg_us: f32 = live.iter().map(|f| f.channel_us[i] as f32).sum::<f32>() / n;
                let max_us = live.iter().map(|f| f.channel_us[i]).max().unwrap_or(0);
                out.push_str(&format!("{}:{{\"avg_us\":{:.0},\"max_us\":{}}}", json_str(name), avg_us, max_us));
            }
            let ts = cx.widget_tree().stats();
            out.push_str(&format!(
                "}},\"tree\":{{\"lookups\":{},\"misses\":{},\"walk_nodes\":{},\"invalidations\":{},\"stores_skipped\":{}}}}}",
                ts.lookups, ts.cache_misses, ts.walk_nodes, ts.invalidations, ts.stores_skipped
            ));
            Ok(out)
        }
        "state" => {
            let (pinned, hover, diff, strokes) = {
                let s = session().lock().unwrap();
                (
                    s.pinned.clone(),
                    s.hover.clone(),
                    s.diff.clone(),
                    s.strokes.clone(),
                )
            };
            let mut out = format!("{{\"on\":{}", if tweak_is_on() { 1 } else { 0 });
            if let Some(r) = session().lock().unwrap().popup {
                out.push_str(&format!(",\"popup\":[{},{},{},{}]", r.pos.x, r.pos.y, r.size.x, r.size.y));
            }
            if let Some(pick) = &pinned {
                out.push_str(",\"sel\":");
                out.push_str(&pick_json(pick));
                // "ref" is the EXACT reference: the indexed path, where every
                // anonymous node carries its position. `path` renders those
                // as a bare `-`, so a run of unnamed containers reads the same
                // for all of them; `ref` is the one to quote and to feed back
                // to /tweak/apply, and it is what a note is keyed by.
                out.push_str(&format!(",\"ref\":{}", json_str(&indexed_path(cx, pick.uid))));
                // Resolve by UID first: paths with anonymous numeric
                // segments (a list item's `demos.1` Slider) do not
                // round-trip through the path finder, but the uid is
                // always exact for the pinned selection.
                let widget = {
                    let by_uid = cx.widget_tree().widget(WidgetUid(pick.uid));
                    if by_uid.is_empty() {
                        resolve_widget_by_path(cx, &pick.path).ok()
                    } else {
                        Some(by_uid)
                    }
                };
                if let Some(widget) = widget {
                    out.push_str(",\"props\":[");
                    for (index, (name, value, is_set)) in
                        reflect_flat(cx, &widget).into_iter().enumerate()
                    {
                        if index > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"n\":{},\"v\":{}{}}}",
                            json_str(&name),
                            json_str(&value),
                            if is_set { ",\"set\":1" } else { "" }
                        ));
                    }
                    out.push(']');
                    out.push_str(",\"cascade\":[");
                    for (index, lvl) in cascade_levels(cx, &widget).iter().enumerate() {
                        if index > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"loc\":{},\"file\":{},\"line\":{}",
                            json_str(&lvl.loc),
                            json_str(&lvl.file),
                            lvl.line
                        ));
                        if let Some(doc) = &lvl.doc {
                            out.push_str(&format!(",\"doc\":{}", json_str(doc)));
                        }
                        if !lvl.field_docs.is_empty() {
                            out.push_str(",\"fields\":{");
                            for (j, (field, doc)) in lvl.field_docs.iter().enumerate() {
                                if j > 0 {
                                    out.push(',');
                                }
                                out.push_str(&format!(
                                    "{}:{}",
                                    json_str(field),
                                    json_str(doc)
                                ));
                            }
                            out.push('}');
                        }
                        out.push_str(",\"sets\":[");
                        for (j, (name, _)) in lvl.sets.iter().enumerate() {
                            if j > 0 {
                                out.push(',');
                            }
                            out.push_str(&json_str(name));
                        }
                        out.push(']');
                        let over: Vec<&String> = lvl
                            .sets
                            .iter()
                            .filter(|(_, overridden)| *overridden)
                            .map(|(name, _)| name)
                            .collect();
                        if !over.is_empty() {
                            out.push_str(",\"over\":[");
                            for (j, name) in over.iter().enumerate() {
                                if j > 0 {
                                    out.push(',');
                                }
                                out.push_str(&json_str(name));
                            }
                            out.push(']');
                        }
                        out.push('}');
                    }
                    out.push(']');
                }
            }
            {
                // Written and waiting, on purpose: NOT requests. An agent
                // sees them so it knows something is being composed, and
                // acts only when they arrive as asks.
                let outbox = session().lock().unwrap().outbox.clone();
                if !outbox.is_empty() {
                    out.push_str(",\"queued\":[");
                    for (i, item) in outbox.iter().enumerate() {
                        let (path, text) = (&item.path, &item.text);
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"path\":{},\"text\":{}}}",
                            json_str(path),
                            json_str(text)
                        ));
                    }
                    out.push(']');
                }
            }
            {
                let renames = {
                    let mut s = session().lock().unwrap();
                    s.load_renames();
                    s.renames.clone()
                };
                if !renames.is_empty() {
                    out.push_str(",\"renames\":[");
                    for (i, rename) in renames.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"ref\":{},\"from\":{},\"to\":{}}}",
                            json_str(&rename.reference),
                            json_str(&rename.from),
                            json_str(&rename.to)
                        ));
                    }
                    out.push(']');
                }
            }
            {
                let converts = {
                    let mut s = session().lock().unwrap();
                    s.load_converts();
                    s.converts.clone()
                };
                if !converts.is_empty() {
                    out.push_str(",\"converts\":[");
                    for (i, convert) in converts.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"ref\":{},\"from\":{},\"to\":{}}}",
                            json_str(&convert.reference),
                            json_str(&convert.from),
                            json_str(&convert.to)
                        ));
                    }
                    out.push(']');
                }
            }
            if let Some(pick) = &hover {
                out.push_str(",\"hover\":");
                out.push_str(&pick_json(pick));
            }
            {
                let notes = {
                    // Pinned notes are part of the state whether or not a
                    // card has been opened this run.
                    let mut s = session().lock().unwrap();
                    s.load_notes();
                    s.notes.clone()
                };
                if !notes.is_empty() {
                    out.push_str(",\"notes\":[");
                    for (i, note) in notes.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        let mentions = note_mentions(&note.path, &note.text);
                        let mentions = if mentions.is_empty() {
                            String::new()
                        } else {
                            format!(
                                ",\"mentions\":[{}]",
                                mentions
                                    .iter()
                                    .map(|m| json_str(m))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            )
                        };
                        out.push_str(&format!(
                            "{{\"path\":{},\"text\":{}{}{}{}}}",
                            json_str(&note.path),
                            json_str(&note.text),
                            // A rule is not a note: an agent reading this
                            // must be able to tell what it may not break
                            // from what it is merely told.
                            if note.rules.trim().is_empty() {
                                String::new()
                            } else {
                                format!(",\"rules\":{}", json_str(&note.rules))
                            },
                            mentions,
                            // "ask": the human pressed Ctrl+Enter / the
                            // sparkle on this note — act on it, do not just
                            // read it. The count rises with every send.
                            if note.sent > 0 { format!(",\"ask\":{}", note.sent) } else { String::new() },
                        ));
                    }
                    out.push(']');
                }
            }
            {
                // The app-wide rules ride alongside the notes, so a standing
                // rule is readable without the app running and without
                // guessing which widget it might have been attached to.
                let rules = {
                    let mut s = session().lock().unwrap();
                    s.load_app_rules();
                    s.app_rules.clone()
                };
                if !rules.trim().is_empty() {
                    out.push_str(&format!(",\"app_rules\":{}", json_str(&rules)));
                }
            }
            {
                let vibes = session().lock().unwrap().vibes.clone();
                if !vibes.is_empty() {
                    out.push_str(",\"vibe\":[");
                    for (i, (vpath, vlayer, vprompt, vfns)) in vibes.iter().enumerate() {
                        if i > 0 {
                            out.push(',');
                        }
                        out.push_str(&format!(
                            "{{\"path\":{},\"layer\":{},\"prompt\":{},\"fns\":{}}}",
                            json_str(vpath),
                            json_str(vlayer),
                            json_str(vprompt),
                            json_str(vfns)
                        ));
                    }
                    out.push(']');
                }
            }
            out.push_str(",\"diff\":");
            out.push_str(&diff_json(&diff));
            out.push_str(",\"ann\":");
            out.push_str(&strokes_json(&strokes));
            // The repaint counter, read WITHOUT causing a frame: two reads
            // apart in time tell whether the window animates on its own
            // (a shader that reads draw_pass.time keeps every live pass
            // repainting — the platform's time repaint, the spinner's too).
            out.push_str(&format!(",\"f\":{}", cx.repaint_id()));
            out.push_str(&format!(",\"shaders\":{}", cx.draw_shaders.shaders.len()));
            {
                let s = session().lock().unwrap();
                out.push_str(",\"states\":[");
                out.push_str(&s.state_names.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(","));
                out.push(']');
                out.push_str(&format!(
                    ",\"states_lock\":{}",
                    s.states_lock.map_or("null".to_string(), |v| format!("{v}"))
                ));
            }
            out.push('}');
            Ok(out)
        }
        "undo" | "redo" => {
            session().lock().unwrap().undo_redo = Some(op == "undo");
            cx.redraw_all();
            Ok(format!("{{\"ok\":1,\"op\":{}}}", json_str(op)))
        }
        "theme" => {
            // Set one global theme value: name=color_x&value=#hex, or a number.
            let name = arg(args, &["name"]).unwrap_or("").to_string();
            let value = arg(args, &["value"]).unwrap_or("").to_string();
            if value.is_empty() {
                // No value: report the theme's current one.
                let current = theme_values(cx)
                    .into_iter()
                    .find(|(n, _, _, _)| *n == name)
                    .map(|(_, _, v, _)| theme_val_text(v));
                return Ok(format!(
                    "{{\"ok\":1,\"theme\":{},\"value\":{}}}",
                    json_str(&name),
                    current.map_or("null".to_string(), |v| json_str(&v))
                ));
            }
            session().lock().unwrap().theme_req = Some((name.clone(), value.clone()));
            cx.redraw_all();
            Ok(format!("{{\"ok\":1,\"theme\":{},\"value\":{}}}", json_str(&name), json_str(&value)))
        }
        "pulse" => {
            // Pin the theme pulse on a colour (name=color_x or a #hex);
            // an empty name restores and unpins.
            let name = arg(args, &["name", "color"]).unwrap_or("").to_string();
            let lock = arg(args, &["m"]).and_then(|s| s.parse::<f32>().ok()).map(|m| m.clamp(0.0, 1.0));
            {
                let mut s = session().lock().unwrap();
                s.pulse_req = Some(name.clone());
                s.pulse_lock = lock;
            }
            cx.redraw_all();
            Ok(format!(
                "{{\"ok\":1,\"pulse\":{},\"m\":{}}}",
                json_str(&name),
                lock.map_or("null".to_string(), |m| format!("{m}"))
            ))
        }
        "states" => {
            // The state swatches' pose lock: phase=0 (off pose), 1 (on
            // pose), any mix between, or auto to animate again.
            let phase = arg(args, &["phase"]).map(|s| s.to_string());
            let mut s = session().lock().unwrap();
            match phase.as_deref() {
                Some("auto") => s.states_lock = None,
                Some(p) => {
                    if let Ok(v) = p.parse::<f64>() {
                        s.states_lock = Some(v.clamp(0.0, 1.0));
                    }
                }
                None => {}
            }
            let lock = s.states_lock;
            let names = s.state_names.clone();
            drop(s);
            cx.redraw_all();
            Ok(format!(
                "{{\"ok\":1,\"lock\":{},\"states\":[{}]}}",
                lock.map_or("null".to_string(), |v| format!("{v}")),
                names.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(",")
            ))
        }
        "apply" => {
            if !tweak_is_on() {
                set_tweak_on(cx, true);
            }
            let path = arg(args, &["path", "p"]).ok_or("need path=")?.to_string();
            let chunk = match arg(args, &["splash", "s", "chunk"]) {
                Some(chunk) => chunk.to_string(),
                None if arg(args, &["const"]).is_some() => String::new(),
                None => {
                    let prop = arg(args, &["prop"]).ok_or("need splash= or prop=+value=")?;
                    let value = arg(args, &["value", "v"]).ok_or("need value=")?;
                    format!("{prop}: {value}")
                }
            };
            let widget = {
                // Anonymous path segments (`-`, list indices) do not round-trip
                // the path finder; the pinned selection resolves by uid, and a
                // caller may pass uid= outright.
                let by_uid = arg(args, &["uid"]).and_then(|s| s.parse::<u64>().ok());
                let pinned = session().lock().unwrap().pinned.clone();
                let w = match (by_uid, pinned) {
                    (Some(uid), _) => cx.widget_tree().widget(WidgetUid(uid)),
                    (None, Some(p)) if p.path == path => cx.widget_tree().widget(WidgetUid(p.uid)),
                    _ => WidgetRef::empty(),
                };
                if w.is_empty() {
                    resolve_widget_by_path(cx, &path)?
                } else {
                    w
                }
            };
            // Applying to a widget selects it — the AI tweaks the very
            // instance the person would see outlined.
            let resolved_path = {
                let uid = widget.widget_uid();
                let ids = cx.widget_tree().path_to(uid);
                if ids.is_empty() {
                    path.clone()
                } else {
                    ids.iter()
                        .map(|id| live_id_token(*id))
                        .collect::<Vec<_>>()
                        .join(".")
                }
            };
            // A shader constant by name: hot-patched on the GPU (reset=1 puts
            // the literal back). No chunk, no recompile.
            if let Some(cname) = arg(args, &["const"]).map(|s| s.to_string()) {
                let reset = arg(args, &["reset"]).is_some_and(|r| r == "1" || r == "true");
                let value = if reset {
                    None
                } else {
                    Some(
                        arg(args, &["value", "v"])
                            .ok_or("need value= (or reset=1)")?
                            .trim()
                            .parse::<f64>()
                            .map_err(|_| "value= must be a number".to_string())?,
                    )
                };
                let (old, new) = const_set(cx, &widget, &resolved_path, &cname, value, "remote")?;
                session().lock().unwrap().pinned = Some(TweakPick {
                    uid: widget.widget_uid().0,
                    path: resolved_path.clone(),
                    ty: String::new(),
                    rect: widget.area().clipped_rect_union(cx),
                    window_id: 0,
                    band: None,
                    level: 0,
                });
                return Ok(format!(
                    "{{\"ok\":1,\"path\":{},\"const\":{},\"old\":{},\"new\":{}}}",
                    json_str(&resolved_path),
                    json_str(&cname),
                    fmt_f64(old as f64),
                    fmt_f64(new as f64)
                ));
            }
            let applied = apply_splash_chunk(cx, &widget, &resolved_path, &chunk, "remote");
            {
                // The prompt's answer landed (or failed): say so in the panel.
                let mut s = session().lock().unwrap();
                if let Some((ppath, _)) = s.vibe_pending.clone() {
                    if ppath == resolved_path || ppath == path {
                        s.vibe_status = match &applied {
                            Ok(_) => "applied \u{2713}".to_string(),
                            Err(e) => format!("error: {e}"),
                        };
                        s.vibe_pending = None;
                    }
                }
            }
            cx.redraw_all();
            let changed = applied?;
            // A fn rewrite from the AI shows in the source view as applied.
            record_fn_overrides(widget.widget_uid().0, &chunk);
            {
                let mut s = session().lock().unwrap();
                let rect = widget.area().clipped_rect_union(cx);
                let ty = widget
                    .widget_type_id()
                    .and_then(|type_id| widget_type_names(cx).get(&type_id).copied())
                    .map(live_id_token)
                    .unwrap_or_else(|| "-".to_string());
                s.pinned = Some(TweakPick {
                    uid: widget.widget_uid().0,
                    path: resolved_path.clone(),
                    ty,
                    rect,
                    window_id: 0,
                    band: None,
                    level: 0,
                });
            }
            Ok(format!(
                "{{\"ok\":1,\"path\":{},\"changed\":{}}}",
                json_str(&resolved_path),
                diff_json(&changed)
            ))
        }
        "diff" => {
            let diff = session().lock().unwrap().diff.clone();
            Ok(format!("{{\"diff\":{}}}", diff_json(&diff)))
        }
        "clear" => {
            let mut s = session().lock().unwrap();
            s.diff.clear();
            s.strokes.clear();
            s.drew = false;
            s.apply_gen += 1;
            Ok("{\"ok\":1}".to_string())
        }
        "final" => {
            let (diff, strokes, drew) = {
                let s = session().lock().unwrap();
                (s.diff.clone(), s.strokes.clone(), s.drew)
            };
            let coalesced = coalesce_diff(&diff);
            Ok(format!(
                "{{\"final\":{},\"ann\":{},\"drew\":{}}}",
                diff_json(&coalesced),
                strokes_json(&strokes),
                if drew { 1 } else { 0 }
            ))
        }
        other => Err(format!("bad tweak op {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// the Tweaker widget — hosted by Window, draws the overlay (outlines, pick
// label, annotation strokes) topmost in the window's own pass, so every
// ordinary grab already composites it.
// ---------------------------------------------------------------------------

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.View

    set_type_default() do #(DrawTweakOutline::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
            sdf.fill_keep(self.fill_color)
            let mut border = self.border_color
            if self.dash > 0.5 {
                // Stipple: dashes along the perimeter, in DEVICE pixels so
                // the pattern reads the same on any display.
                let p = self.pos * self.rect_size * self.dpi
                let t = modf(p.x + p.y, 8.0)
                if t > 4.0 {
                    border = vec4(0.0, 0.0, 0.0, 0.0)
                }
            }
            sdf.stroke(border, self.border_size)
            return sdf.result
        }
    }

    set_type_default() do #(DrawTweakChecker::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            // The alpha-visualizing checkerboard: 6px two-tone squares.
            let p = floor(self.pos * self.rect_size / 6.0)
            let t = modf(p.x + p.y, 2.0)
            return mix(vec4(0.42, 0.42, 0.42, 1.0), vec4(0.58, 0.58, 0.58, 1.0), t)
        }
    }

    set_type_default() do #(DrawTweakStroke::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.rect_size.x * 0.5)
            sdf.fill(self.stroke_color)
            return sdf.result
        }
    }

    mod.widgets.TweakMaterialSwatchBase = #(TweakMaterialSwatch::register_widget(vm))
    mod.widgets.TweakMaterialSwatch = set_type_default() do mod.widgets.TweakMaterialSwatchBase{
        width: Fill
        height: 34
    }

    mod.widgets.TweakerBase = #(Tweaker::register_widget(vm))
    mod.widgets.Tweaker = set_type_default() do mod.widgets.TweakerBase{
        width: 0
        height: 0
        draw_label +: {
            text_style +: {
                font_size: 7.5
            }
            color: #xffffff
        }
        draw_label_bg +: {
            color: #x1a2733dd
        }
        draw_splitter +: {
            color: #x161616
        }
        draw_panel_bg +: {
            color: #x303030
        }
    }
}

/// A live material preview: a quad drawn with the SELECTION'S OWN draw
/// shader. The tweaker applies the selected widget's current draw-layer
/// value onto `preview` (same source, same instance values), so the
/// fn-hash shader cache compiles to the identical shader — the swatch IS
/// the material, not a color approximation. Quad-family layers only
/// (guarded by a rect_pos probe); other layers draw the default flat quad.
#[derive(Script, ScriptHook, Widget)]
pub struct TweakMaterialSwatch {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    preview: DrawQuad,
    #[live]
    checker: DrawTweakChecker,
    /// The widget whose material this swatch mirrors (0 = none): the well
    /// re-reads that widget's live draw call every time it draws, so a
    /// scrub on the widget shows in the well the same frame.
    #[rust]
    mirror_uid: u64,
    #[rust]
    mirror_layer: String,
    /// True when `mirror_layer` is the widget's primary material — the one
    /// its area belongs to (draw_bg for most, draw_slider for a Slider…).
    #[rust]
    mirror_primary: bool,
    /// The well's own draw list: the mirrored instance is drawn at the
    /// widget's NATIVE size (so px-sized shader internals — a checkbox's
    /// 14px mark box, a border width — stay true) and the list's view
    /// transform magnifies it into the well. A magnifier, not a stretch.
    #[rust]
    well_list: Option<DrawList2d>,
    #[rust]
    area: Area,
    /// Which draw layer this swatch currently previews.
    #[rust]
    pub layer: String,
    /// Rebuild generation the preview was last applied at (MAX = never).
    #[rust(u64::MAX)]
    pub applied_gen: u64,
    /// The scroll viewport this swatch lives in (screen coords): the
    /// mirrored draw call is clipped to it (its own draw list escapes the
    /// scroll view's clipping otherwise).
    #[rust]
    pub clip: Option<Rect>,
    /// When set, the swatch shows ONE animator track of the mirrored
    /// widget: the layer's instance slice is posed between the track's off
    /// and on apply values by `mix` (0 = off, 1 = on) before it draws.
    #[rust]
    pub state: Option<StateTrack>,
    #[rust]
    pub mix: f32,
}

impl Widget for TweakMaterialSwatch {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let turtle_rect = cx.turtle().rect();
        let aligned = self.checker.area().rect(cx);
        let rect = if aligned.size.x > 0.0 && aligned.size.y > 0.0 { aligned } else { turtle_rect };
        self.checker.draw_abs(cx, turtle_rect);
        // Same shader, same inputs: copy the widget's live draw call (its
        // shader id, uniforms, textures and this instance's values —
        // colours, border sizes, hover/focus/down mix factors as they are
        // right now) and draw one instance of it here with only the
        // geometry substituted. "you do kinda have to feed it similar
        // inputs as the actual widget probably like colors and things".
        let mirrored = if self.mirror_uid != 0 {
            let widget = cx.widget_tree().widget(WidgetUid(self.mirror_uid));
            // The layer's own area when the widget exposes it (every
            // `#[live] Draw…` field does), else the widget's area for its
            // primary material.
            let layer_area = widget
                .layer_areas()
                .into_iter()
                .find(|(name, _)| *name == self.mirror_layer)
                .map(|(_, a)| a)
                .or_else(|| if self.mirror_primary { Some(widget.area()) } else { None });
            layer_area.and_then(|area| capture_material_mirror(cx, &widget, area, &self.preview.draw_vars))
        } else {
            None
        };
        match mirrored {
            Some(mut mirror) => {
                if let Some(state) = &self.state {
                    state.pose(cx, &mut mirror, self.mix);
                }
                let native = mirror.native_size();
                if self.well_list.is_none() {
                    self.well_list = Some(DrawList2d::new(cx));
                }
                let list = self.well_list.as_mut().unwrap();
                // The swatch only draws when its panel redraws, and every such
                // draw is a new capture: the well list must redraw with it, or
                // it keeps showing the previous selection's material.
                let well_id = list.id();
                cx.redraw_list(well_id);
                if list.begin(cx, Walk::abs_rect(rect)).is_redrawing() {
                    // Integer zoom when it fits, a shrink when it does not.
                    let fit = (rect.size.x / native.x.max(1.0)).min(rect.size.y / native.y.max(1.0));
                    let k = if fit >= 1.0 { fit.floor().min(8.0) } else { fit };
                    let tx = rect.pos.x + (rect.size.x - native.x * k) * 0.5;
                    let ty = rect.pos.y + (rect.size.y - native.y * k) * 0.5;
                    let m = Mat4f {
                        v: [
                            k as f32, 0.0, 0.0, 0.0, //
                            0.0, k as f32, 0.0, 0.0, //
                            0.0, 0.0, 1.0, 0.0, //
                            tx as f32, ty as f32, 0.0, 1.0,
                        ],
                    };
                    let id = list.id();
                    let uniforms_gen = cx.next_uniform_gen();
                    cx.draw_lists[id].set_uniform_view_transform(&m, uniforms_gen);
                    // The scroll viewport's clip, seen through the
                    // magnifier: (screen - t) / k.
                    let clip = self.clip.map(|c| {
                        (
                            dvec2((c.pos.x - tx) / k, (c.pos.y - ty) / k),
                            dvec2((c.pos.x + c.size.x - tx) / k, (c.pos.y + c.size.y - ty) / k),
                        )
                    });
                    let visible = clip.map_or(true, |(min, max)| max.x > min.x.max(0.0) && max.y > min.y.max(0.0) && min.x < native.x && min.y < native.y);
                    if visible {
                        mirror.draw(cx, Rect { pos: dvec2(0.0, 0.0), size: native }, clip);
                    }
                    list.end(cx);
                }
            }
            None => self.preview.draw_abs(cx, rect),
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

/// The checkerboard behind material swatches: translucent shaders read
/// against it (the standard alpha backdrop).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweakChecker {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweakOutline {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub border_color: Vec4f,
    #[live]
    pub fill_color: Vec4f,
    #[live]
    pub border_size: f32,
    /// 1.0 = stippled (the tweaking-in-progress hairline).
    #[live]
    pub dash: f32,
    /// Device pixels per point, for dpi-true hairlines and dashes.
    #[live(1.0)]
    pub dpi: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweakStroke {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub stroke_color: Vec4f,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Num,
    Bool,
    Text,
    Color,
    /// Read-only display row (the CASCADE section): label + value only.
    Info,
}

/// The sidebar's property groups, hottest first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    Layout,
    Style,
    Text,
    Behavior,
    Other,
    /// The construction chain of the selection: one block per prototype
    /// level (instance -> styled type -> base -> native), each with its
    /// source location, `///` docs and the keys it sets. CSS-inspector
    /// style: a key later marked ~name~ is overridden by a closer level.
    Cascade,
}

const SECTION_ORDER: [SectionKind; 6] = [
    SectionKind::Layout,
    SectionKind::Style,
    SectionKind::Text,
    SectionKind::Behavior,
    SectionKind::Other,
    SectionKind::Cascade,
];

impl SectionKind {
    fn index(self) -> usize {
        match self {
            SectionKind::Layout => 0,
            SectionKind::Style => 1,
            SectionKind::Text => 2,
            SectionKind::Behavior => 3,
            SectionKind::Other => 4,
            SectionKind::Cascade => 5,
        }
    }
    fn title(self) -> &'static str {
        match self {
            SectionKind::Layout => "Layout",
            SectionKind::Style => "Style",
            SectionKind::Text => "Text",
            SectionKind::Behavior => "Behavior",
            SectionKind::Other => "Other",
            SectionKind::Cascade => "Cascade",
        }
    }
}

/// Derive the group from what the reflection already knows — never a
/// hand-tagged schema. A known family matched by name prefix beats OTHER.
fn classify_prop(prop: &str, value: &str) -> SectionKind {
    let first = prop.split('.').next().unwrap_or("");
    match first {
        "width" | "height" | "abs_pos" | "margin" | "padding" | "spacing" | "line_spacing"
        | "align" | "flow" | "clip_x" | "clip_y" | "scroll" | "wrap_spacing" | "layout"
        | "metrics" | "distribute" | "container_id" | "min_width" | "max_width"
        | "min_height" | "max_height" | "aspect" | "cell" | "columns" | "rows" | "areas"
        | "column_gap" | "row_gap" | "auto_flow" | "justify_items" | "align_items"
        | "implicit_column_size" | "implicit_row_size" => return SectionKind::Layout,
        "text" | "empty_text" | "label" | "title" | "suffix" => return SectionKind::Text,
        "visible" | "enabled" | "grab_key_focus" | "cursor" | "trigger_on_press"
        | "enable_long_press" | "reset_hover_on_click" | "block_signal_event"
        | "capture_overload" | "event_order" | "design_mode" | "skip_widget_tree_search"
        | "grab_focus" | "hover_actions_enabled" | "animator" => return SectionKind::Behavior,
        _ => {}
    }
    if first.ends_with("_walk") {
        // label_walk belongs with the text it lays out; the rest are layout.
        if first == "label_walk" {
            return SectionKind::Text;
        }
        return SectionKind::Layout;
    }
    if first.starts_with("draw_") || first == "text_style" || first == "icon" {
        return SectionKind::Style;
    }
    if value == "true" || value == "false" {
        return SectionKind::Behavior;
    }
    SectionKind::Other
}

/// Inset legs in the box-model order: left, top, right, bottom.
fn inset_leg_rank(leaf: &str) -> u32 {
    match leaf {
        "left" => 0,
        "top" => 1,
        "right" => 2,
        "bottom" => 3,
        _ => 4,
    }
}

/// The within-section ordering: (major, minor, alpha tail). Equal keys keep
/// reflection order (stable sort) — and changed rows never reorder at all.
fn section_rank(section: SectionKind, prop: &str) -> (u32, u32, String) {
    let mut parts = prop.split('.');
    let first = parts.next().unwrap_or("");
    let leaf = parts.next().unwrap_or("");
    match section {
        // Cascade rows are pushed in display order after the sort and keep
        // it via the stable sort's equal-key rule.
        SectionKind::Cascade => (0, 0, String::new()),
        SectionKind::Layout => {
            // The box-model progression, outside-in.
            let major = match first {
                "width" => 0,
                "height" => 1,
                "min_width" => 2,
                "max_width" => 3,
                "min_height" => 4,
                "max_height" => 5,
                "aspect" => 6,
                "abs_pos" => 7,
                "cell" => 8,
                "margin" => 9,
                "padding" => 10,
                "spacing" => 11,
                "wrap_spacing" => 12,
                "line_spacing" => 13,
                "align" => 14,
                "distribute" => 15,
                "flow" => 16,
                "clip_x" => 17,
                "clip_y" => 18,
                "scroll" => 19,
                "container_id" => 20,
                "columns" => 21,
                "rows" => 22,
                "column_gap" => 23,
                "row_gap" => 24,
                "auto_flow" => 25,
                "areas" => 26,
                "justify_items" => 27,
                "align_items" => 28,
                "implicit_column_size" => 29,
                "implicit_row_size" => 29,
                _ => 30,
            };
            let minor = match first {
                "margin" | "padding" => inset_leg_rank(leaf),
                "align" => match leaf {
                    "x" => 0,
                    "y" => 1,
                    _ => 2,
                },
                _ => 0,
            };
            (major, minor, prop.to_string())
        }
        SectionKind::Style => {
            // Surface first, then content layers; colors lead each family.
            let major = match first {
                "draw_bg" => 0,
                "draw_text" => 1,
                "draw_icon" => 2,
                _ => 3, // remaining draw_* families keep reflection order
            };
            let minor = if leaf == "color" {
                0
            } else if first == "draw_bg" {
                match leaf {
                    "border_color" => 2,
                    "border_size" => 3,
                    "border_radius" => 4,
                    _ if leaf.starts_with("color") => 1,
                    _ if leaf.starts_with("border_color") => 2,
                    _ => 10,
                }
            } else if first == "draw_text" {
                if leaf.starts_with("color") {
                    1
                } else if leaf == "text_style" || leaf.contains("font") {
                    2
                } else {
                    10
                }
            } else if first == "draw_icon" {
                if leaf.starts_with("color") {
                    1
                } else if leaf == "scale" {
                    2
                } else {
                    10
                }
            } else if leaf.starts_with("color") {
                1
            } else {
                10
            };
            (major, minor, prop.to_string())
        }
        SectionKind::Text => {
            let major = match first {
                "text" => 0,
                "empty_text" => 1,
                "label" => 2,
                "title" => 3,
                "suffix" => 4,
                "label_walk" => 5,
                "metrics" => 6,
                _ => 10,
            };
            (major, 0, prop.to_string())
        }
        SectionKind::Behavior => {
            let major = match first {
                "visible" => 0,
                "enabled" => 1,
                "grab_key_focus" | "grab_focus" => 2,
                "trigger_on_press" | "enable_long_press" | "reset_hover_on_click" => 3,
                "animator" => 4,
                _ => 10,
            };
            (major, 0, prop.to_string())
        }
        SectionKind::Other => (0, 0, prop.to_string()),
    }
}

/// One sidebar row: the property it edits and the uids of the field widgets
/// whose actions carry the edits.
#[derive(Clone)]
struct RowBinding {
    prop: String,
    kind: RowKind,
    value: String,
    /// The string value was quoted in reflection (a real string property).
    quoted: bool,
    section: SectionKind,
    /// Set at the instance level (own map) — the cascade cue: inherited
    /// values render with a dimmer label.
    set: bool,
    /// This property differs from its session-original (resettable).
    changed: bool,
    /// The session-original value (the first diff entry's `old`).
    original: Option<String>,
    field_uid: u64,
    /// The color row's swatch (hex box is `field_uid`).
    swatch_uid: u64,
    /// A structured value's typed editor (vec/inset/metrics/size); None for
    /// scalar rows.
    struct_kind: StructKind,
    /// Structured value's live component values (vec x/y/z/w, inset legs…).
    comp_vals: Vec<f64>,
    /// The uids of the component number fields, in component order.
    comp_uids: Vec<u64>,
    /// Field uids of this row's top-section copy: (uid, is_swatch).
    alt_uids: Vec<(u64, bool)>,
    /// A shader-constant row: an annotated literal inside a draw layer's
    /// fn body (Cx::shader_const_table), hot-patched on the GPU — never a
    /// chunk apply.
    const_ref: Option<ConstRef>,
    /// The theme colour this row's value equals, when one does.
    theme_match: Option<String>,
    /// The row's "≈ theme.color_x" button uid (click = use the reference).
    theme_uid: u64,
}

/// One hot-patchable shader constant of the pinned widget's draw layer.
#[derive(Clone)]
struct ConstRef {
    layer: String,
    name: String,
    initial: f32,
}

/// One visible sidebar entry, with the rects the raw-pointer gestures
/// (section fold, label double-click reset) hit-test against.
/// The doc tooltip a hovered row shows (text + pointer position).
#[derive(Clone, PartialEq)]
struct HoverDoc {
    text: String,
    pos: Vec2d,
}

/// The side panel's tabs.
#[derive(Clone, Copy, PartialEq, Default)]
enum PanelTab {
    #[default]
    Props,
    Shader,
    Tree,
    /// The global theme: its colours, spacing and font sizes, edited live.
    Theme,
    /// What is WRITTEN about the selection -- its notes and its rules -- and
    /// the rules that stand over the whole app. The app-wide field is why
    /// this tab has to work with nothing selected.
    Spec,
}

#[derive(Clone, Copy, PartialEq)]
enum BoxKind {
    Margin,
    Padding,
}

impl BoxKind {
    fn prop(self) -> &'static str {
        match self {
            BoxKind::Margin => "margin",
            BoxKind::Padding => "padding",
        }
    }
}

#[derive(Clone, Copy)]
enum VisKind {
    Section(SectionKind, usize, bool),
    Prop(usize),
    /// width + height compacted onto one row.
    Size,
    /// The measured rect, as one full-width line above the size controls.
    Measured,
    /// The selection's identity at the very top: its name, editable, and its
    /// type. Everything below is what it LOOKS like; this is what it IS.
    Identity,
    /// Four-sided box editor (mini rectangle, drag-to-scrub legs).
    BoxInset(BoxKind),
    /// spacing + flow on one row.
    FlowSpacing,
    /// justify (main axis, with the distribution) and align (cross axis).
    AlignGrid,
    /// A heading over the rows that are about the selection in its parent,
    /// or over the rows that are about its children.
    Group(GroupKind),
    /// flex or grid, the ask to become the other, and the container name.
    Container,
    /// Out of the flow, at a position in the parent.
    Absolute,
    /// A Grid's tracks, gaps, fill order and named areas.
    GridTracks,
    /// Where the selection sits in its parent Grid.
    Cell,
    /// "show all (N)": the section's long tail, folded by default.
    More(SectionKind, usize),
    /// A material card header: layer name + live shader preview swatch
    /// (index into Tweaker::materials).
    Material(usize),
    /// The TWEAKABLES header at the top: every annotated value, hottest
    /// first, with its doc under it — the designer's surface.
    TweakHeader(usize, bool),
    /// An annotated row rendered inside TWEAKABLES (same template as Prop;
    /// the row keeps a second set of field uids for it).
    Tweakable(usize),
    /// The annotation line (doc + range) under a row.
    /// The Shader tab's INPUTS header: the mirrored layer's own inputs.
    InputsHeader(usize),
    /// One level of the selection's cascade (index into Tweaker::cascade).
    CascadeLevel(usize),
}

/// Which of the Layout section's two halves a heading opens: the rows
/// about the selection where it sits, or the rows about what it holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GroupKind {
    Item,
    Container,
}

impl VisKind {
    /// Same composite row? Only the kinds `composite_of` names ever reach
    /// this, so the box editors compare their side and the rest their tag.
    fn same_row(&self, other: &VisKind) -> bool {
        match (self, other) {
            (VisKind::Size, VisKind::Size)
            | (VisKind::FlowSpacing, VisKind::FlowSpacing)
            | (VisKind::AlignGrid, VisKind::AlignGrid)
            | (VisKind::Container, VisKind::Container)
            | (VisKind::Absolute, VisKind::Absolute)
            | (VisKind::GridTracks, VisKind::GridTracks)
            | (VisKind::Cell, VisKind::Cell) => true,
            (VisKind::BoxInset(a), VisKind::BoxInset(b)) => a == b,
            (VisKind::Group(a), VisKind::Group(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone)]
struct VisRow {
    kind: VisKind,
    /// The drawn row item; its area answers hit tests at gesture time
    /// (areas only become queryable after the frame commits).
    item: WidgetRef,
}

#[derive(Script, Widget)]
pub struct Tweaker {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[rust]
    overlay_list: Option<DrawList2d>,
    #[redraw]
    #[live]
    draw_outline: DrawTweakOutline,
    #[live]
    draw_stroke: DrawTweakStroke,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_label_bg: DrawColor,
    #[live]
    draw_splitter: DrawColor,
    /// Opaque backing for the whole sidebar band — the panel must read as
    /// one solid surface whatever the app's clear color is.
    #[live]
    draw_panel_bg: DrawColor,
    /// The property sidebar, built lazily at first open from a runtime
    /// splash chunk (every widget type is registered by then, whatever the
    /// registration order was).
    #[rust]
    sidebar: Option<WidgetRef>,
    #[rust]
    rows: Vec<RowBinding>,
    /// Selection uid the rows were last built for.
    #[rust]
    rows_uid: u64,
    /// The apply generation the rows were last built at.
    #[rust]
    rows_gen: u64,
    /// The visible entries (sections + rows after filter/fold), with the
    /// rects the raw-pointer gestures hit-test against. Rebuilt each draw.
    #[rust]
    visible: Vec<VisRow>,
    /// Live substring filter (the pinned search box), lowercase.
    #[rust]
    filter: String,
    /// Folded sections (ignored while the filter is active).
    #[rust]
    collapsed: [bool; 6],
    /// The search box's input uid, captured at draw.
    #[rust]
    search_uid: u64,
    /// Double-click detection on row labels: (time, row index).
    #[rust]
    last_label_click: Option<(f64, usize)>,
    /// While a text field is being live-edited: (row index, the value when
    /// focus began — Escape restores it).
    #[rust]
    text_edit_origin: Option<(usize, String)>,
    /// The radius-like style input of the selection, if any: the corner
    /// handles drive it. First of the direct-manipulation handle family.
    #[rust]
    radius_prop: Option<(String, f64)>,
    /// An in-flight corner-handle drag: (corner 0..3, start value, start pos).
    #[rust]
    radius_drag: Option<(usize, f64, Vec2d)>,
    /// Re-check the outline suppression window when it expires.
    #[rust]
    next_frame: NextFrame,
    /// Long-tail expansion per section ("show all (N)" clicked).
    #[rust]
    expanded: [bool; 6],
    /// Composite-row fields drawn this frame: (widget uid, prop it edits).
    /// Value/text changes from these uids apply like ordinary rows.
    #[rust]
    composite_fields: Vec<(u64, String)>,
    /// Segment buttons drawn this frame: (uid, splash chunk they apply).
    #[rust]
    composite_clicks: Vec<(u64, String)>,
    /// The selection's draw layers, in row order (material card headers).
    #[rust]
    materials: Vec<String>,
    /// Doc-channel text per row prop (tooltips + scrubber hints).
    #[rust]
    row_docs: HashMap<String, String>,
    /// The doc tooltip shown on row hover.
    #[rust]
    hover_doc: Option<HoverDoc>,
    /// Which cascade level (0 = instance) set each top-level prop.
    #[rust]
    origin_levels: HashMap<String, usize>,
    /// The selection's construction chain, one entry per level.
    #[rust]
    cascade: Vec<CascadeLevel>,
    /// How many cascade levels the selection has (colors + scroll target).
    #[rust]
    cascade_level_count: usize,
    /// Focus the filter input on the next sidebar draw ('/' or tweak-on).
    #[rust]
    focus_search_pending: bool,
    /// The exploded-view toggle button beside the filter.
    #[rust]
    sploded_uid: u64,
    /// The select toggle: lit while the overlay is picking, dark while the
    /// selection is locked and the mouse belongs to the app.
    #[rust]
    select_uid: u64,
    /// The exploded view's level-separation scrub field (visible only
    /// while the mode is up).
    spread_uid: u64,
    /// The extrusion readout, floating in the app's own top-right corner
    /// rather than in the panel: it belongs to the picture it is changing,
    /// and the eye is on the stack, not on the sidebar, while it moves.
    #[rust]
    spread_ui: Option<WidgetRef>,
    /// Where it drew, so the pointer knows it is chrome and not canvas.
    #[rust]
    spread_rect: Option<Rect>,
    /// A scrub of that field is in progress: the pointer belongs to it until
    /// the button comes up, wherever it travels.
    #[rust]
    spread_drag: bool,
    /// The scope toggle's buttons.
    #[rust]
    scope_this_uid: u64,
    #[rust]
    scope_all_uid: u64,
    /// The "isolated" modifier beside them: confine an `all` fan-out to the
    /// isolated branch.
    #[rust]
    scope_isolated_uid: u64,
    /// The Shader tab's editor and the live-code loop: every change arms a
    /// short debounce; at settle the fn text is applied through the ledger
    /// (one entry per settle, like a scrub gesture). A compile error keeps
    /// the last good text running and shows the message under the editor.
    #[rust]
    shader_src_uid: u64,
    /// The source editor stays folded until asked for: the Shader tab
    /// opens on the swatch + doc + prompt, and this button uid toggles the
    /// source view open.
    #[rust]
    shader_fold_uid: u64,
    #[rust]
    shader_src_open: bool,
    #[rust]
    live_timer: Timer,
    #[rust]
    live_last_applied: String,
    #[rust]
    live_last_good: String,
    #[rust]
    live_error_pending: bool,
    /// (widget, layer) the source view currently holds text for.
    #[rust]
    live_key: (u64, String),
    /// The layer doc in full when the terse line had to cut it.
    #[rust]
    shader_doc_full: String,
    #[rust]
    doc_tip_shown: bool,
    /// Which scope button the tooltip is up for (0 = none, 1 = this, 2 = all).
    #[rust]
    scope_tip_shown: u8,
    /// The text the panel's own control under the pointer last showed in
    /// that same bubble; empty when none. See `chrome_tip_hover`.
    #[rust]
    chrome_tip_shown: String,
    /// The tooltip's measured size. It sits ABOVE the buttons because the
    /// footer is pinned to the bottom of the panel and anything below the
    /// row would be off the window; and it is pulled left to stay inside the
    /// window, because a scope button near the panel's right edge would
    /// otherwise push it off the side. Measured after the first show; the
    /// fallback is only ever used for one frame.
    #[rust(dvec2(246.0, 46.0))]
    scope_tip_size: Vec2d,
    #[rust]
    fn_external_seen: u64,
    /// The shown layer's script-defined fns: (name, file:line, source).
    vibe_fn_sources: Vec<(String, String, String)>,
    /// An apply just happened: redraw the panel one frame later so the
    /// swatch mirrors the widget AFTER it redrew with the new value.
    swatch_refresh: bool,
    /// The pinned widget's animator tracks, shown as posed state swatches
    /// under the well.
    #[rust]
    state_tracks: Vec<StateTrack>,
    #[rust]
    states_gen: u64,
    #[rust]
    states_uid: u64,
    #[rust]
    states_layer: String,
    #[rust]
    states_paused: bool,
    #[rust]
    states_hover: bool,
    #[rust]
    states_t0: f64,
    #[rust]
    states_frame: NextFrame,
    #[rust]
    states_pause_uid: u64,
    /// SHADER CONSTANTS fold state (open by default: it is the point).
    #[rust(true)]
    tweakables_open: bool,
    /// The Shader tab's row list (constants + inputs of the mirrored layer).
    #[rust]
    shader_list_uid: u64,
    #[rust]
    shader_entries: Vec<VisKind>,
    /// The row whose field was last touched: its doc line rides under it.
    #[rust]
    doc_row: Option<usize>,
    /// The Shader tab's scroll viewport, handed to every well it hosts.
    /// Captured at event time: mid-draw the rect slots answer zero.
    #[rust]
    swatch_clip: Option<Rect>,
    /// The Props list's viewport (props_wrap below the scope control).
    #[rust]
    props_viewport: Option<Rect>,
    /// The theme's colour palette (name, rgba, defined-at), read when the
    /// sidebar is built and again after a theme edit made elsewhere.
    #[rust]
    theme_colors: Vec<(String, u32, String)>,
    /// The session's apply generation the palette was read at.
    #[rust]
    palette_gen: u64,
    /// The colour being hover-pulsed app-wide (and when it started).
    #[rust]
    pulse: Option<(u32, f64)>,
    /// Pinned by a remote `op=pulse`: the pointer no longer clears it.
    #[rust]
    pulse_pinned: bool,
    #[rust]
    pulse_ticks: u64,
    #[rust]
    pulse_last_sync: f64,
    /// The theme's definition site (file:line of its first value).
    #[rust]
    theme_site: String,
    #[rust]
    pulse_frame: NextFrame,
    /// A scroll-to-selection servo: each tree draw reports where the
    /// selected row landed and the error is corrected until it is inside
    /// the viewport (estimates and clamping cannot diverge it).
    #[rust]
    tree_scroll_tries: Option<u8>,
    /// Set by the note button; the next event shows the Spec tab.
    note_request: bool,
    /// Armed state for the 2.5D exploded z-layer view (M3 wires the
    /// renderer; until then this is the mode flag + visual state).
    #[rust]
    sploded_armed: bool,
    /// Which side-panel tab is active (persists across Shift+F10).
    #[rust]
    panel_tab: PanelTab,
    /// The shader tab's draw layer (clicking a material thumbnail switches
    /// the tab here and sets this).
    #[rust]
    vibe_layer: Option<String>,
    /// Tab-bar button uids, captured at draw.
    #[rust]
    tab_uids: [u64; 5],
    /// The two PortalLists' uids (props, tree), captured at ensure.
    #[rust]
    props_list_uid: u64,
    #[rust]
    tree_list_uid: u64,
    /// The flattened widget tree for the tree tab + its generation.
    #[rust]
    tree_rows: Vec<crate::widget_tree::FlatTreeRow>,
    #[rust]
    tree_rows_gen: u64,
    /// Tree rows drawn this frame: (item, target widget uid).
    #[rust]
    tree_visible: Vec<(WidgetRef, u64)>,
    /// Set while the hover outline was driven from the tree tab.
    #[rust]
    tree_hover_active: bool,
    /// The selection uid the tree last auto-scrolled to (scroll once per
    /// selection change; never fight the user's own scrolling).
    #[rust]
    tree_scrolled_uid: u64,
    /// Parent index per tree row (same order as tree_rows).
    #[rust]
    tree_parents: Vec<Option<usize>>,
    /// Child indices per tree row.
    #[rust]
    tree_children: Vec<Vec<usize>>,
    /// Isolate: the Tree tab lists ONLY the selection and what is inside it.
    /// A whole app's widget tree is thousands of rows; when the question is
    /// about one pane, the rest is noise to scroll past.
    #[rust]
    tree_isolate: bool,
    /// WHICH widget isolation is locked to. Captured when the toggle goes on
    /// and held until it goes off — selecting a child inside an isolated
    /// subtree must not re-isolate onto the child, or every click would
    /// narrow the view and you could never look at the thing you opened.
    #[rust]
    isolate_uid: u64,
    /// The Isolate toggle's own uid.
    #[rust]
    tree_isolate_uid: u64,
    /// Center: hold the isolated widget in the middle of the screen.
    #[rust]
    view_center: bool,
    #[rust]
    view_center_uid: u64,
    /// Magnification, 1..4. The wheel drives it too while isolated.
    #[rust]
    view_zoom: f32,
    /// The wheel moved the zoom mid-dispatch; push it onto the view at the
    /// next event, where a `&mut Cx` is to hand.
    #[rust]
    view_focus_pending: bool,
    #[rust]
    view_zoom_uid: u64,
    /// Open the readable default levels once per tree refresh.
    #[rust]
    tree_open_defaults_pending: bool,
    /// The prompt TextInput's uid, captured at draw.
    #[rust]
    #[rust]
    note_text_uid: u64,
    /// The identity row's name field.
    #[rust]
    identity_uid: u64,
    /// The container row's `make grid` / `make flex` button.
    #[rust]
    convert_uid: u64,
    /// The absolute row's checkbox.
    #[rust]
    abs_uid: u64,
    /// The selection is a Grid: its children's half shows tracks, not a
    /// flow. It sits in a Grid: its own half shows a cell.
    #[rust]
    sel_is_grid: bool,
    #[rust]
    sel_in_grid: bool,
    /// The footer's copy receipt is shown until this time: a click on the
    /// path line put it on the clipboard, and that has to be visible.
    #[rust]
    footer_copied_until: f64,
    /// How many `@`s the note text held at the last change: one more means
    /// a mention was just armed, one fewer means it was taken back.
    #[rust]
    note_at_count: usize,
    /// Pinned notes' badge widgets: (uid, note key), refreshed on a slow
    /// timer rather than every frame — resolving a note path walks the whole
    /// widget tree, and the rects are read live off the uids anyway.
    #[rust]
    badge_targets: Vec<(u64, String, BadgeKind)>,
    /// When `badge_targets` was last resolved.
    #[rust]
    badges_at: f64,
    /// Where the badges landed this frame, for hit-testing the click that
    /// opens one.
    #[rust]
    badge_rects: Vec<(Rect, u64)>,
    /// The pin badge itself: one Icon, drawn once per badge.
    #[rust]
    badge_ui: Option<[WidgetRef; 3]>,
    /// A badge click, consumed by the tweaker's event loop.
    #[rust]
    badge_open: Option<u64>,
    /// The selection's indexed path, cached by uid. Computing it walks the
    /// whole widget tree, so it happens when the selection CHANGES, not on
    /// every frame that reads it.
    #[rust]
    sel_ref: (u64, String),
    /// The strip's box has been emptied by a send: clear the TextInput on
    /// the next draw rather than fighting it mid-keystroke.
    #[rust]
    prompt_clear_field: bool,
    #[rust]
    prompt_queue_uid: u64,
    #[rust]
    prompt_send_uid: u64,
    /// The prompt box's own uid, so an `@` typed into it arms a pick the
    /// same way one typed into the notes field does.
    #[rust]
    prompt_field_uid: u64,
    /// Put the caret back in the box the frame after a send or a queue. A
    /// message going out must not cost you the box you were writing in:
    /// without this, Up after a send walks nothing, because the strip's keys
    /// are claimed only while the caret is actually there.
    #[rust]
    prompt_focus_pending: bool,
    /// A splitter being dragged: which boundary (0 is notes/rules, 1 is
    /// rules/app), where the press landed, the three weights as they were
    /// at the press, and how much weight one point of pointer travel is
    /// worth. The weights are the only truth. An earlier version measured
    /// the boxes on screen at each press and turned pixels back into
    /// weights, and that snapped: the walk lands a frame after the weight
    /// changes, the measured rects a frame after that, so the second handle
    /// grabbed was reading a split that was already gone.
    #[rust]
    spec_resize: Option<(usize, f64, [f64; 3], f64)>,
    /// The weights last pushed at the boxes. An apply per frame would be
    /// three applies per frame forever; this makes it three per drag step.
    #[rust]
    spec_applied_w: [f64; 3],
    /// The cross that empties the notes field.
    #[rust]
    spec_clear_uid: u64,
    /// Something in the Spec tab was typed into and is not on disk yet.
    /// Flushed when the caret leaves the tab's fields -- a write per
    /// keystroke would be a file write per frame.
    #[rust]
    spec_dirty: bool,
    /// The prompt box's own `@` count -- see `note_at_count`, and why they
    /// are two: one counter for two fields let typing in one arm or disarm
    /// the other's mention, and a disarmed mention click falls through to a
    /// pick and moves the selection the pending ask is about.
    #[rust]
    prompt_at_count: usize,
    /// Which note the Spec tab is actually SHOWING. The tab follows the
    /// selection, so the note being saved to and the text on screen can drift
    /// apart for a frame — and a save in that window would write one note's
    /// text over another's. Nothing is written unless these agree.
    #[rust]
    note_key_shown: String,
    /// Previous tweak-mode state, to detect the on edge.
    #[rust]
    was_on: bool,
    /// The open color-picker popover's rect: input inside it belongs to
    /// the popup — row gestures skip it and the scroll list ignores wheel
    /// there (the input-side mirror of app < outlines < panel < popups).
    #[rust]
    open_popup: Option<Rect>,
    /// Linked-toggle state of the two box editors (margin, padding): a
    /// change to one leg applies to all four.
    #[rust]
    box_link: [bool; 2],
    /// The two link-toggle uids (margin, padding).
    #[rust]
    box_link_uids: [u64; 2],
    /// The panel's own overlay draw list: begun AFTER the outline overlay,
    /// so the stacking is app < outlines < panel < panel-popups — the app
    /// (dock tab bars included) can never read through the panel.
    #[rust]
    sidebar_list: Option<DrawList2d>,
    /// The sidebar band (splitter included), window-local, cached at draw.
    #[rust]
    band: Rect,
    /// This widget's window, learned at draw time.
    #[rust]
    my_window: Option<usize>,
    /// The margin-right currently applied to the window body.
    #[rust]
    applied_margin: f64,
    /// The body's own margin.right before the panel compressed it.
    #[rust]
    saved_body_right: Option<f64>,
    #[rust]
    splitter_drag: bool,
}

impl ScriptHook for Tweaker {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.overlay_list = Some(DrawList2d::script_new(vm));
        self.sidebar_list = Some(DrawList2d::script_new(vm));
    }
}

impl Tweaker {
    fn redraw_overlay(&mut self, cx: &mut Cx) {
        if let Some(overlay_list) = &self.overlay_list {
            overlay_list.redraw(cx);
        }
    }

    /// The window body is this widget's sibling in the window view; find it
    /// through the widget tree (zeros in the path are anonymous hops the
    /// finder treats as wildcards anyway, drop them).
    fn find_body(&self, cx: &Cx) -> Option<WidgetRef> {
        let tree = cx.widget_tree();
        let mut path = tree.path_to(self.uid);
        path.pop()?; // "tweaker"
        path.retain(|id| *id != LiveId(0));
        path.push(live_id!(body));
        let found = tree.find_within(tree.root_uid(), &path);
        if found.is_empty() {
            None
        } else {
            Some(found)
        }
    }

    /// Compress (or release) the app's UI: the body gets a right margin the
    /// size of the sidebar band, through the ordinary apply machinery so the
    /// relayout is the real one. Not a user edit — never enters the diff.
    fn ensure_body_margin(&mut self, cx: &mut Cx, desired: f64) {
        if (self.applied_margin - desired).abs() < 0.5 {
            return;
        }
        let Some(body) = self.find_body(cx) else {
            return;
        };
        // A dotted `margin.right:` apply fails when the body's margin is a
        // scalar (an f64 has no .right). Read the current legs (scalar
        // margins fan out to all four) and apply one full Inset; remember
        // the body's own right so release restores it, not zero.
        let before = reflect_flat(cx, &body);
        let leg = |name: &str| {
            before
                .iter()
                .find(|(n, _, _)| n == name)
                .and_then(|(_, v, _)| v.parse::<f64>().ok())
        };
        let scalar = leg("margin");
        let left = leg("margin.left").or(scalar).unwrap_or(0.0);
        let top = leg("margin.top").or(scalar).unwrap_or(0.0);
        let bottom = leg("margin.bottom").or(scalar).unwrap_or(0.0);
        if self.saved_body_right.is_none() {
            self.saved_body_right = Some(leg("margin.right").or(scalar).unwrap_or(0.0));
        }
        let right = if desired > 0.5 {
            desired
        } else {
            self.saved_body_right.take().unwrap_or(0.0)
        };
        let chunk = format!(
            "margin: Inset{{left: {left} top: {top} right: {right:.0} bottom: {bottom}}}"
        );
        match eval_chunk(cx, &body, &chunk) {
            Ok(()) => {
                self.applied_margin = desired;
                cx.redraw_all();
            }
            Err(error) => {
                log!("TWEAK body compress failed: {error}");
                // Don't retry every frame.
                self.applied_margin = desired;
            }
        }
    }

    /// Build the sidebar widget from a runtime splash chunk, once (every
    /// widget type — the fab controls included — is registered by then).
    fn ensure_sidebar(&mut self, cx: &mut Cx) {
        if self.sidebar.is_some() {
            return;
        }
        if self.theme_colors.is_empty() {
            self.theme_colors = theme_palette(cx);
            self.palette_gen = session().lock().unwrap().apply_gen;
            log!("TWEAK theme palette: {} colours", self.theme_colors.len());
        }
        // The shader source view is a plain multiline TextInput, on purpose:
        // the real code editor as a sidebar child would put a CodeView in
        // the main window's widget tree for every app the tweaker rides in.
        // Live-coding needs only text-in/text-out — Ctrl+Enter and the
        // settle timer read `.text()` by path, whatever widget holds it.
        let sidebar = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*

                // Row templates, hoisted: one source of truth for the Props list,
                // the Shader tab INPUTS list and the shader-constant rows.
                let SectionRowT = FabSection {
                    count := FabLabelSmall { width: Fit margin: Inset{left: 4 top: 1 right: 0 bottom: 0} text: "" }
                }
                let CascadeRowT = View {
                    width: Fill
                    height: Fit
                    flow: Down
                    spacing: 2
                    padding: Inset{left: 8 right: 8 top: 4 bottom: 4}
                    head := View {
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 6
                        align: Align{x: 0.0 y: 0.5}
                        ic_app := View { width: Fit height: Fit visible: false
                            i := Icon { icon_walk: Walk{width: 12 height: Fit} draw_icon +: { color: #xbbbbbb svg: crate_resource("self:resources/icons/icon_file.svg") } }
                        }
                        ic_lib := View { width: Fit height: Fit visible: false
                            i := Icon { icon_walk: Walk{width: 12 height: Fit} draw_icon +: { color: #xbbbbbb svg: crate_resource("self:resources/icons/icon_widget.svg") } }
                        }
                        ic_theme := View { width: Fit height: Fit visible: false
                            i := Icon { icon_walk: Walk{width: 12 height: Fit} draw_icon +: { color: #xbbbbbb svg: crate_resource("self:resources/icons/icon_draw.svg") } }
                        }
                        ic_native := View { width: Fit height: Fit visible: false
                            i := Icon { icon_walk: Walk{width: 12 height: Fit} draw_icon +: { color: #xbbbbbb svg: crate_resource("self:resources/icons/icon_layout.svg") } }
                        }
                        chip := RoundedView {
                            width: Fit height: Fit
                            padding: Inset{left: 5 right: 5 top: 1 bottom: 1}
                            draw_bg +: { color: #x555555 radius: 3. }
                            lbl := FabLabelSmall { width: Fit text: "L0" draw_text +: { color: #x151515 } }
                        }
                        loc := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    }
                    sets_wrap := View { width: Fill height: Fit visible: false
                        sets := FabLabelSmall { width: Fill margin: Inset{left: 22 top: 0 right: 0 bottom: 0} text: "" }
                    }
                    overridden_wrap := View { width: Fill height: Fit visible: false
                        overridden := FabLabelSmall { width: Fill margin: Inset{left: 22 top: 0 right: 0 bottom: 0} text: "" }
                    }
                }
                let MaterialRowT = View {
                    width: Fill
                    height: 40
                    flow: Right
                    spacing: 6
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 3 bottom: 3}
                    name := mod.widgets.FabLabelDim {
                        width: 70
                        text: ""
                    }
                    swatch_bg := View {
                        width: Fill
                        height: 30
                        show_bg: true
                        padding: Inset{left: 2 right: 2 top: 2 bottom: 2}
                        draw_bg +: {
                            color: #x606060
                        }
                        swatch := TweakMaterialSwatch {
                            width: Fill
                            height: Fill
                        }
                    }
                }
                let NumRowT = View {
                    width: Fill
                    height: 24
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    // Tight on the right: every point between the swatch and
                    // the panel edge is a point the NAME does not get, and
                    // the name is the thing that was being truncated.
                    padding: Inset{left: 8 right: 2 top: 0 bottom: 0}
                    spacing: 4
                    name := FabLabelDim {
                        width: Fill
                        text: ""
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                    // Sized to the value column, not to the value: the
                    // widest thing that has to fit anywhere in it is a
                    // `#00000000`, and every point past that is a point
                    // stolen from the name, which is what gets truncated.
                    value := FabValueInput {
                        width: 106
                        height: 18
                    }
                    origin := FabLabelSmall { width: 8 margin: Inset{left: 0 top: 2 right: 0 bottom: 0} text: "" }
                }
                let BoolRowT = View {
                    width: Fill
                    height: 24
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    // Tight on the right: every point between the swatch and
                    // the panel edge is a point the NAME does not get, and
                    // the name is the thing that was being truncated.
                    padding: Inset{left: 8 right: 2 top: 0 bottom: 0}
                    spacing: 4
                    name := FabLabelDim {
                        width: Fill
                        text: ""
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                    value := CheckBox {
                        width: Fit
                        height: Fit
                        text: ""
                    }
                    origin := FabLabelSmall { width: 8 margin: Inset{left: 0 top: 2 right: 0 bottom: 0} text: "" }
                }
                let TextRowT = View {
                    width: Fill
                    height: 24
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    // Tight on the right: every point between the swatch and
                    // the panel edge is a point the NAME does not get, and
                    // the name is the thing that was being truncated.
                    padding: Inset{left: 8 right: 2 top: 0 bottom: 0}
                    spacing: 4
                    name := FabLabelDim {
                        width: Fill
                        text: ""
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                    value := TextInput {
                        width: 106
                        height: 18
                        empty_text: ""
                        draw_bg +: {
                            color: #x1d1d1d
                            border_radius: 2.0
                        }
                        draw_text +: {
                            ink_centered: true
                            color: #xe6e6e6
                            text_style +: {
                                font_size: 8.5
                            }
                        }
                    }
                    origin := FabLabelSmall { width: 8 margin: Inset{left: 0 top: 2 right: 0 bottom: 0} text: "" }
                }
                let InfoRowT = FabPropRow {
                    value := FabLabelSmall {
                        width: Fill
                        margin: Inset{left: 0 top: 2 right: 0 bottom: 0}
                        text: ""
                    }
                }
                // One size field, in the person's own words: a number, a
                // percentage, an expression.
                let SizeInputT = TextInput {
                    height: 18
                    empty_text: ""
                    label_align: Align{x: 0.5 y: 0.5}
                    draw_bg +: {
                        color: #x1d1d1d
                        border_radius: 2.0
                    }
                    draw_text +: {
                        ink_centered: true
                        color: #xe6e6e6
                        text_style +: { font_size: 8.5 }
                    }
                }
                let SizeRowT = FabPropRow {
                    height: Fit
                    size_col := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        w_row := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            w_axis := FabLabelSmall { width: 12 text: "W" }
                            w_seg := View { width: Fit height: Fit flow: Right spacing: 1
                                w_fill := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                w_fit := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                w_fix := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                            }
                            w_input := SizeInputT { width: Fill }
                        }
                        // The content-box clamps, under the axis they bound.
                        w_clamp := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            w_min_label := FabLabelSmall { width: Fit text: "min" }
                            w_min := SizeInputT { width: 42 empty_text: "\u{2013}" }
                            w_max_label := FabLabelSmall { width: Fit margin: Inset{left: 4 top: 0 right: 0 bottom: 0} text: "max" }
                            w_max := SizeInputT { width: 42 empty_text: "\u{2013}" }
                        }
                        // A Fill's own fields; the lines are not there otherwise.
                        w_grow := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            w_grow_label := FabLabelSmall { width: Fit text: "grow" }
                            w_weight := FabValueInput { width: 42 height: 18 precision: 1 padding: Inset{left: 4 right: 4 top: 0 bottom: 0} }
                            w_shrink_label := FabLabelSmall { width: Fit margin: Inset{left: 4 top: 0 right: 0 bottom: 0} text: "shrink" }
                            w_shrink := FabValueInput { width: 42 height: 18 precision: 1 padding: Inset{left: 4 right: 4 top: 0 bottom: 0} }
                        }
                        w_basis := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            w_basis_label := FabLabelSmall { width: Fit text: "basis" }
                            w_basis_in := SizeInputT { width: Fill }
                        }
                        h_row := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            h_axis := FabLabelSmall { width: 12 text: "H" }
                            h_seg := View { width: Fit height: Fit flow: Right spacing: 1
                                h_fill := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                h_fit := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                h_fix := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                            }
                            h_input := SizeInputT { width: Fill }
                        }
                        // The content-box clamps, under the axis they bound.
                        h_clamp := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            h_min_label := FabLabelSmall { width: Fit text: "min" }
                            h_min := SizeInputT { width: 42 empty_text: "\u{2013}" }
                            h_max_label := FabLabelSmall { width: Fit margin: Inset{left: 4 top: 0 right: 0 bottom: 0} text: "max" }
                            h_max := SizeInputT { width: 42 empty_text: "\u{2013}" }
                        }
                        // A Fill's own fields; the lines are not there otherwise.
                        h_grow := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            h_grow_label := FabLabelSmall { width: Fit text: "grow" }
                            h_weight := FabValueInput { width: 42 height: 18 precision: 1 padding: Inset{left: 4 right: 4 top: 0 bottom: 0} }
                            h_shrink_label := FabLabelSmall { width: Fit margin: Inset{left: 4 top: 0 right: 0 bottom: 0} text: "shrink" }
                            h_shrink := FabValueInput { width: 42 height: 18 precision: 1 padding: Inset{left: 4 right: 4 top: 0 bottom: 0} }
                        }
                        h_basis := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            h_basis_label := FabLabelSmall { width: Fit text: "basis" }
                            h_basis_in := SizeInputT { width: Fill }
                        }
                        aspect_row := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            aspect_label := FabLabelSmall { width: Fit text: "aspect" }
                            aspect_in := SizeInputT { width: 42 empty_text: "\u{2013}" }
                            aspect_hint := FabLabelSmall { width: Fit text: "width : height" }
                        }
                    }
                }
                // What the size fields ASK for is Fill / Fit / a number; this
                // says what the layout actually gave. Its own full-width line
                // at the top of the Layout section, above the controls it
                // reports on, so the numbers are not squeezed into a column.
                // The selection's identity: a name you can change and the
                // type you cannot. The name is a TextInput because renaming
                // an anonymous widget is the commonest thing to want to say
                // about it; the type is a label because it is a fact.
                let IdentityRowT = View {
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 6
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 8 top: 3 bottom: 5}
                    name_field := TextInput {
                        // 70 / 30 against the type beside it: a name is
                        // usually short, and a type that gets ellipsised is
                        // no use at all.
                        width: Fill{weight: 70.0}
                        height: 20
                        empty_text: "unnamed \u{2014} type a name"
                        draw_bg +: {
                            color: #x1d1d1d
                            border_radius: 2.0
                        }
                        draw_text +: {
                            color: #xe6e6e6
                            text_style +: { font_size: 8.5 }
                        }
                    }
                    type_label := FabLabelDim {
                        width: Fill{weight: 30.0}
                        text: ""
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                }
                let MeasuredRowT = View {
                    width: Fill
                    height: Fit
                    flow: Right
                    padding: Inset{left: 8 right: 8 top: 1 bottom: 3}
                    measured := FabLabelSmall {
                        width: Fill
                        text: ""
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                }
                let BoxRowT = FabPropRow {
                    height: Fit
                    margin: Inset{left: 0 right: 0 top: 3 bottom: 9}
                    box_col := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        top_row := View {
                            width: Fill
                            height: Fit
                            align: Align{x: 0.5 y: 0.5}
                            leg_top := FabValueInput { width: 64 height: 16 }
                        }
                        mid_row := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.5 y: 0.5}
                            leg_left := FabValueInput { width: 64 height: 16 }
                            frame := View {
                                width: Fill
                                height: 20
                                show_bg: true
                                draw_bg +: {
                                    pixel: fn() {
                                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                                        sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, 3.0)
                                        sdf.fill_keep(#x26262600)
                                        sdf.stroke(#x5a5a5a, 1.0)
                                        return sdf.result
                                    }
                                }
                            }
                            leg_right := FabValueInput { width: 64 height: 16 }
                        }
                        bot_row := View {
                            width: Fill
                            height: Fit
                            align: Align{x: 0.5 y: 0.5}
                            leg_bottom := FabValueInput { width: 64 height: 16 }
                        }
                    }
                    link := CheckBox {
                        width: Fit
                        height: Fit
                        text: ""
                    }
                }
                // The container's flow on two lines: the direction, the
                // wrap and how a wrapped row lines up; then the gaps.
                let FlowRowT = FabPropRow {
                    height: Fit
                    flow_col := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        dir_row := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            flow_seg := View { width: Fit height: Fit flow: Right spacing: 1
                                f_right := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                f_down := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                f_over := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                            }
                            f_wrap := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                            ra_seg := View { width: Fit height: Fit flow: Right spacing: 1 margin: Inset{left: 4 right: 0 top: 0 bottom: 0}
                                ra_top := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                ra_mid := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                ra_bottom := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                            }
                        }
                        gap_row := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            gap_label := FabLabelSmall { width: Fit text: "gap" }
                            spacing_input := FabValueInput { width: 56 height: 18 }
                            wrap_box := View { width: Fit height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                                wrap_label := FabLabelSmall { width: Fit margin: Inset{left: 4 top: 0 right: 0 bottom: 0} text: "rows" }
                                wrap_input := FabValueInput { width: 56 height: 18 }
                            }
                        }
                    }
                }
                // The children along the flow and across it. Which is x
                // and which is y follows the direction, so the labels say.
                let AlignRowT = FabPropRow {
                    height: Fit
                    align_col := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        just_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            just_label := FabLabelSmall { width: 34 text: "justify" }
                            just_seg := View { width: Fit height: Fit flow: Right spacing: 1
                                j_stretch := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                j_start := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                j_mid := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                j_end := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                            }
                            just_axis := FabLabelSmall { width: Fit text: "" }
                        }
                        space_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            space_label := FabLabelSmall { width: 34 text: "space" }
                            space_seg := View { width: Fit height: Fit flow: Right spacing: 1
                                s_between := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                s_around := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                s_evenly := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                            }
                        }
                        cross_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            cross_label := FabLabelSmall { width: 34 text: "align" }
                            cross_seg := View { width: Fit height: Fit flow: Right spacing: 1
                                c_stretch := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                c_start := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                c_mid := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                c_end := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                            }
                            cross_axis := FabLabelSmall { width: Fit text: "" }
                        }
                    }
                }
                // A heading inside the Layout section: the rows under it are
                // about the selection in its parent, or about its children.
                let GroupRowT = View {
                    width: Fill
                    height: 20
                    flow: Right
                    align: Align{x: 0.0 y: 1.0}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 2}
                    title := FabLabelSmall { width: Fill text: "" }
                }
                // What kind of container it is, the ask to become the other
                // kind, and the name its children can size against.
                let ContainerRowT = FabPropRow {
                    height: Fit
                    ctr_col := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        mode_row := View { width: Fill height: Fit flow: Right spacing: 6 align: Align{x: 0.0 y: 0.5}
                            mode_label := FabLabelSmall { width: Fit text: "" }
                            convert := Button { width: Fit height: Fit padding: Inset{left: 4 right: 4 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                        }
                        name_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            ctr_label := FabLabelSmall { width: Fit text: "named" }
                            ctr_name := SizeInputT { width: Fill empty_text: "\u{2013}" label_align: Align{x: 0.0 y: 0.5} draw_text +: { ink_centered: false } }
                        }
                    }
                }
                // Out of the flow: a checkbox, and the place in the parent
                // once it is.
                let AbsRowT = FabPropRow {
                    height: Fit
                    abs_col := View {
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 6
                        align: Align{x: 0.0 y: 0.5}
                        abs_check := CheckBox { width: Fit height: Fit text: "" }
                        abs_label := FabLabelSmall { width: Fit text: "absolute" }
                        abs_xy := View { width: Fit height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            x_label := FabLabelSmall { width: Fit text: "x" }
                            abs_x := FabValueInput { width: 44 height: 18 }
                            y_label := FabLabelSmall { width: Fit text: "y" }
                            abs_y := FabValueInput { width: 44 height: 18 }
                        }
                    }
                }
                // A Grid's own layout: the tracks as CSS, the gaps, which way
                // cells without a place are filled in, and the named areas.
                let GridRowT = FabPropRow {
                    height: Fit
                    grid_col := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        cols_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            cols_label := FabLabelSmall { width: 40 text: "columns" }
                            cols_in := SizeInputT { width: Fill label_align: Align{x: 0.0 y: 0.5} draw_text +: { ink_centered: false } }
                        }
                        rows_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            rows_label := FabLabelSmall { width: 40 text: "rows" }
                            rows_in := SizeInputT { width: Fill label_align: Align{x: 0.0 y: 0.5} draw_text +: { ink_centered: false } }
                        }
                        gaps_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            gap_label := FabLabelSmall { width: 34 text: "gap" }
                            gap_x := FabLabelSmall { width: Fit text: "\u{2194}" }
                            gap_col := FabValueInput { width: 40 height: 18 }
                            gap_y := FabLabelSmall { width: Fit margin: Inset{left: 4 top: 0 right: 0 bottom: 0} text: "\u{2195}" }
                            gap_row_in := FabValueInput { width: 40 height: 18 }
                        }
                        fill_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            fill_label := FabLabelSmall { width: 40 text: "fill" }
                            fill_seg := View { width: Fit height: Fit flow: Right spacing: 1
                                f_rows := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                                f_cols := Button { width: Fit height: Fit padding: Inset{left: 3 right: 3 top: 1 bottom: 1} margin: Inset{left:0 right:0 top:0 bottom:0} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                            }
                        }
                        areas_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            areas_label := FabLabelSmall { width: 40 text: "areas" }
                            areas_in := SizeInputT { width: Fill label_align: Align{x: 0.0 y: 0.5} draw_text +: { ink_centered: false } }
                        }
                    }
                }
                // Where a child of a Grid sits: a column and row (0 is wherever
                // the fill order puts it), how many of each it spans, or an
                // area by name.
                let CellRowT = FabPropRow {
                    height: Fit
                    cell_col := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        place_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            col_label := FabLabelSmall { width: Fit text: "col" }
                            cell_c := FabValueInput { width: 40 height: 18 min: 0.0 step: 1.0 precision: 0 }
                            row_label := FabLabelSmall { width: Fit margin: Inset{left: 4 top: 0 right: 0 bottom: 0} text: "row" }
                            cell_r := FabValueInput { width: 40 height: 18 min: 0.0 step: 1.0 precision: 0 }
                        }
                        span_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            span_label := FabLabelSmall { width: Fit text: "span" }
                            cell_cs := FabValueInput { width: 40 height: 18 min: 0.0 step: 1.0 precision: 0 }
                            by_label := FabLabelSmall { width: Fit text: "\u{00d7}" }
                            cell_rs := FabValueInput { width: 40 height: 18 min: 0.0 step: 1.0 precision: 0 }
                        }
                        area_row := View { width: Fill height: Fit flow: Right spacing: 4 align: Align{x: 0.0 y: 0.5}
                            area_label := FabLabelSmall { width: Fit text: "area" }
                            cell_area := SizeInputT { width: Fill empty_text: "\u{2013}" label_align: Align{x: 0.0 y: 0.5} draw_text +: { ink_centered: false } }
                        }
                    }
                }
                let MoreRowT = FabSection {}
                let ColorRowT = View {
                    width: Fill
                    height: 24
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    // Tight on the right: every point between the swatch and
                    // the panel edge is a point the NAME does not get, and
                    // the name is the thing that was being truncated.
                    padding: Inset{left: 8 right: 2 top: 0 bottom: 0}
                    spacing: 4
                    name := FabLabelDim {
                        width: Fill
                        text: ""
                        max_lines: 1
                        text_overflow: TextOverflow.Ellipsis
                    }
                    // 80 + spacing + the swatch is exactly the 106 the number
                    // rows use, so both columns end on the same edge — and 80
                    // is what `#00000000` actually needs.
                    value := TextInput {
                        width: 80
                        height: 18
                        empty_text: "#rrggbbaa"
                        draw_bg +: {
                            color: #x1d1d1d
                            border_radius: 2.0
                        }
                        draw_text +: {
                            ink_centered: true
                            color: #xe6e6e6
                            text_style +: {
                                font_size: 8.5
                            }
                        }
                    }
                    swatch := FabColorPick {
                        width: 22
                        height: 16
                    }
                    tname_wrap := View { width: Fit height: Fit visible: false
                        tname := Button { width: Fit height: 16 padding: Inset{left: 4 right: 4 top: 1 bottom: 1} text: "" draw_text +: { text_style +: { font_size: 7.0 } } }
                    }
                    origin := FabLabelSmall { width: 8 margin: Inset{left: 0 top: 2 right: 0 bottom: 0} text: "" }
                }
                let VecRowT = View {
                    width: Fill height: 24 flow: Right align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0} spacing: 4
                    name := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    vx := FabValueInput { width: 46 height: 18 }
                    vy := FabValueInput { width: 46 height: 18 }
                    vz_wrap := View { width: Fit height: Fit visible: false
                        vz := FabValueInput { width: 46 height: 18 }
                    }
                    vw_wrap := View { width: Fit height: Fit visible: false
                        vw := FabValueInput { width: 46 height: 18 }
                    }
                }
                let InsetRowT = View {
                    width: Fill height: 24 flow: Right align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0} spacing: 3
                    name := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    il := FabValueInput { width: 40 height: 18 }
                    it := FabValueInput { width: 40 height: 18 }
                    ir := FabValueInput { width: 40 height: 18 }
                    ib := FabValueInput { width: 40 height: 18 }
                }
                let MetricsRowT = View {
                    width: Fill height: 24 flow: Right align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0} spacing: 4
                    name := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    m0 := FabValueInput { width: 44 height: 18 }
                    m1 := FabValueInput { width: 44 height: 18 }
                    m2 := FabValueInput { width: 44 height: 18 }
                }
                let NoEditorRowT = View {
                    width: Fill height: 24 flow: Right align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 0 bottom: 0} spacing: 6
                    name := FabLabelDim { width: Fill text: "" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                    ne := FabLabelSmall { width: Fit text: "no editor yet" }
                }
                View {
                    width: Fill
                    height: Fill
                    flow: Down
                    show_bg: true
                    draw_bg +: {
                        color: #x303030
                    }
                    padding: Inset{left: 4 right: 4 top: 6 bottom: 4}
                    spacing: 4
                    filter_row := View {
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 4
                        align: Align{x: 0.0 y: 0.5}
                        search := FabSearch {}
                        select := Button {
                            width: 28
                            height: 24
                            padding: Inset{left: 6 right: 6 top: 4 bottom: 4}
                            margin: Inset{left: 0 right: 4 top: 0 bottom: 0}
                            text: ""
                            icon_walk: Walk{width: 13 height: Fit}
                            draw_icon +: {
                                color: #xd8d8d8
                                svg: crate_resource("self:resources/icons/icon_select.svg")
                            }
                        }
                        sploded := Button {
                            width: 28
                            height: 24
                            padding: Inset{left: 5 right: 5 top: 3 bottom: 3}
                            margin: Inset{left: 0 right: 4 top: 0 bottom: 0}
                            text: ""
                            icon_walk: Walk{width: 15 height: Fit}
                            draw_icon +: {
                                color: #xd8d8d8
                                svg: crate_resource("self:resources/icons/sploded.svg")
                            }
                        }
                    }
                    tab_row := View {
                        width: Fill
                        height: 22
                        flow: Right
                        spacing: 2
                        padding: Inset{left: 4 right: 4 top: 0 bottom: 0}
                        tab_props := Button { width: Fit height: 20 padding: Inset{left: 8 right: 8 top: 2 bottom: 2} text: "Props" draw_text +: { text_style +: { font_size: 8.0 } } }
                        tab_shader := Button { width: Fit height: 20 padding: Inset{left: 8 right: 8 top: 2 bottom: 2} text: "Shader" draw_text +: { text_style +: { font_size: 8.0 } } }
                        tab_tree := Button { width: Fit height: 20 padding: Inset{left: 8 right: 8 top: 2 bottom: 2} text: "Tree" draw_text +: { text_style +: { font_size: 8.0 } } }
                        tab_theme := Button { width: Fit height: 20 padding: Inset{left: 8 right: 8 top: 2 bottom: 2} text: "Theme" draw_text +: { text_style +: { font_size: 8.0 } } }
                        tab_spec := Button { width: Fit height: 20 padding: Inset{left: 8 right: 8 top: 2 bottom: 2} text: "Spec" draw_text +: { text_style +: { font_size: 8.0 } } }
                    }
                    shader_col := ScrollYView {
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 6
                        padding: Inset{left: 8 right: 8 top: 6 bottom: 6}
                        shader_title := FabHeaderLabel {
                            width: Fill
                            text: ""
                        }
                        shader_doc := FabLabelSmall {
                            width: Fill
                            text: ""
                            max_lines: 1
                            text_overflow: TextOverflow.Ellipsis
                        }
                        big := TweakMaterialSwatch {
                            width: Fill
                            height: 150
                        }
                        states_row := View {
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 8
                            visible: false
                            states_pause := Button { width: Fit height: 18 padding: Inset{left: 6 right: 6 top: 1 bottom: 1} text: "pause" draw_text +: { text_style +: { font_size: 8.0 } } }
                            st0 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                            st1 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                            st2 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                            st3 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                            st4 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                            st5 := View { width: Fill height: Fit flow: Down spacing: 2 visible: false
                                lbl := FabLabelSmall { text: "" }
                                sw := TweakMaterialSwatch { width: Fill height: 150 }
                            }
                        }
                        shader_rows_wrap := View {
                            width: Fill
                            height: Fit
                            visible: false
                        shader_rows := PortalList {
                            width: Fill
                            height: 320
                            margin: Inset{left: 0 top: 2 right: 0 bottom: 0}
                            drag_scrolling: false
                            SectionRow := SectionRowT {}
                            MaterialRow := MaterialRowT {}
                            NumRow := NumRowT {}
                            BoolRow := BoolRowT {}
                            TextRow := TextRowT {}
                            InfoRow := InfoRowT {}
                            SizeRow := SizeRowT {}
                            MeasuredRow := MeasuredRowT {}
                            IdentityRow := IdentityRowT {}
                            BoxRow := BoxRowT {}
                            FlowRow := FlowRowT {}
                            AlignRow := AlignRowT {}
                            GroupRow := GroupRowT {}
                            ContainerRow := ContainerRowT {}
                            AbsRow := AbsRowT {}
                            GridRow := GridRowT {}
                            CellRow := CellRowT {}
                            MoreRow := MoreRowT {}
                            ColorRow := ColorRowT {}
                            VecRow := VecRowT {}
                            InsetRow := InsetRowT {}
                            MetricsRow := MetricsRowT {}
                            NoEditorRow := NoEditorRowT {}
                        }
                        }
                        src_fold := Button {
                            width: Fit
                            height: 20
                            padding: Inset{left: 8 right: 8 top: 2 bottom: 2}
                            text: "+ source"
                            draw_text +: { text_style +: { font_size: 8.0 } }
                        }
                        src_scroll := View {
                            width: Fill
                            height: Fit
                            show_bg: true
                            draw_bg +: { color: #x1b1b1b }
                            padding: Inset{left: 6 right: 6 top: 4 bottom: 4}
                            shader_src := TextInput {
                                width: Fill
                                height: Fit
                                is_multiline: true
                                draw_bg +: { color: #x1b1b1b }
                                draw_text +: {
                                    color: #xd0d0d0
                                    text_style: theme.font_code
                                    text_style +: { font_size: 7.5 }
                                }
                            }
                        }
                        doc_tip := Tooltip {
                            width: 0
                            height: 0
                            clip_x: false
                            clip_y: false
                            content := RoundedView {
                                width: Fit
                                height: Fit
                                padding: Inset{left: 8 right: 8 top: 6 bottom: 6}
                                draw_bg +: {
                                    color: #x2a2a2a
                                    border_size: 1.0
                                    border_color: #x555555
                                    radius: 3.
                                }
                                tooltip_label := FabLabelSmall {
                                    width: 220
                                    text: ""
                                }
                            }
                        }
                    }
                    spec_col := View {
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 4
                        padding: Inset{left: 8 right: 8 top: 6 bottom: 6}
                        spec_for := FabLabelSmall {
                            width: Fill
                            text: ""
                            max_lines: 1
                            text_overflow: TextOverflow.Ellipsis
                        }
                        notes_head := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            notes_label := FabHeaderLabel {
                                width: Fill
                                text: "notes"
                            }
                            notes_clear := Button {
                                width: 15
                                height: 15
                                padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
                                margin: Inset{left: 0 right: 0 top: 0 bottom: 0}
                                align: Align{x: 0.5 y: 0.5}
                                text: "\u{00d7}"
                                draw_bg +: { color: #x00000000 }
                                draw_text +: {
                                    color: #x8a8a94
                                    text_style +: { font_size: 10.0 }
                                }
                            }
                        }
                        notes_box := View {
                            width: Fill
                            height: Fill{weight: 1.0 min: 48.0}
                            spec_notes := TextInput {
                                width: Fill
                                height: Fill
                                is_multiline: true
                                empty_text: "what this widget is about \u{2014} for whoever reads it next"
                                draw_bg +: {
                                    color: #x1b1b1b
                                    border_radius: 3.0
                                }
                                draw_text +: {
                                    color: #xe6e6e6
                                    text_style +: { font_size: 8.5 }
                                }
                            }
                        }
                        rules_head := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            rules_label := FabHeaderLabel {
                                width: Fill
                                text: "rules"
                            }
                            grip := RoundedView {
                                width: 28
                                height: 3
                                margin: Inset{left: 0 right: 4 top: 0 bottom: 0}
                                draw_bg +: { color: #x5c5c68 radius: 1.5 }
                            }
                        }
                        rules_box := View {
                            width: Fill
                            height: Fill{weight: 1.0 min: 48.0}
                            spec_rules := TextInput {
                                width: Fill
                                height: Fill
                                is_multiline: true
                                empty_text: "what must stay true of this widget"
                                draw_bg +: {
                                    color: #x1b1b1b
                                    border_radius: 3.0
                                }
                                draw_text +: {
                                    color: #xe6e6e6
                                    text_style +: { font_size: 8.5 }
                                }
                            }
                        }
                        app_head := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            app_label := FabHeaderLabel {
                                width: Fill
                                text: "app rules"
                            }
                            grip := RoundedView {
                                width: 28
                                height: 3
                                margin: Inset{left: 0 right: 4 top: 0 bottom: 0}
                                draw_bg +: { color: #x5c5c68 radius: 1.5 }
                            }
                        }
                        app_box := View {
                            width: Fill
                            height: Fill{weight: 1.0 min: 48.0}
                            spec_app := TextInput {
                                width: Fill
                                height: Fill
                                is_multiline: true
                                empty_text: "rules that stand over the whole app \u{2014} no selection needed"
                                draw_bg +: {
                                    color: #x1b1b1b
                                    border_radius: 3.0
                                }
                                draw_text +: {
                                    color: #xe6e6e6
                                    text_style +: { font_size: 8.5 }
                                }
                            }
                        }
                    }
                    tree_wrap := View {
                        width: Fill
                        height: Fill
                        flow: Down
                        tree_head := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 4
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 8 right: 8 top: 3 bottom: 3}
                            isolate := Button {
                                width: Fit
                                height: 18
                                padding: Inset{left: 8 right: 8 top: 1 bottom: 1}
                                text: "Isolate"
                                draw_text +: { text_style +: { font_size: 8.0 } }
                            }
                            center := Button {
                                width: Fit
                                height: 18
                                padding: Inset{left: 8 right: 8 top: 1 bottom: 1}
                                text: "Center"
                                draw_text +: { text_style +: { font_size: 8.0 } }
                            }
                            zoom_label := FabLabelSmall { width: Fit text: "zoom" }
                            // 1.00 is life size and the floor: below it the
                            // view would shrink the app away from the very
                            // detail Zoom exists to bring closer.
                            zoom := FabValueInput { width: 44 height: 18 min: 1.0 max: 4.0 }
                            isolate_hint := FabLabelSmall {
                                width: Fill
                                text: ""
                                max_lines: 1
                                text_overflow: TextOverflow.Ellipsis
                            }
                        }
                        tree := FileTree {}
                    }
                    props_wrap := View {
                        width: Fill
                        height: Fill
                        flow: Down
                        props := PortalList {
                        width: Fill
                        height: Fill
                        margin: Inset{left: 0 top: 2 right: 0 bottom: 0}
                        // A desktop inspector full of draggable controls
                        // cannot share press-drags with its scroller:
                        // content drag-scroll is a touch idiom and it
                        // fought every field scrub for the gesture. Wheel
                        // and the scrollbar thumb still scroll.
                        drag_scrolling: false
                        SectionRow := SectionRowT {}
                        MaterialRow := MaterialRowT {}
                        NumRow := NumRowT {}
                        BoolRow := BoolRowT {}
                        TextRow := TextRowT {}
                        InfoRow := InfoRowT {}
                        CascadeRow := CascadeRowT {}
                        SizeRow := SizeRowT {}
                        MeasuredRow := MeasuredRowT {}
                        IdentityRow := IdentityRowT {}
                        BoxRow := BoxRowT {}
                        FlowRow := FlowRowT {}
                        AlignRow := AlignRowT {}
                        GroupRow := GroupRowT {}
                        ContainerRow := ContainerRowT {}
                        AbsRow := AbsRowT {}
                        GridRow := GridRowT {}
                        CellRow := CellRowT {}
                        MoreRow := MoreRowT {}
                        ColorRow := ColorRowT {}
                        VecRow := VecRowT {}
                        InsetRow := InsetRowT {}
                        MetricsRow := MetricsRowT {}
                        NoEditorRow := NoEditorRowT {}
                    }
                        app_grip := View {
                            width: Fill
                            height: 6
                            align: Align{x: 0.5 y: 0.5}
                            // A plain View's draw_bg is a bare DrawQuad whose
                            // default pixel fn returns #0000, so show_bg plus a
                            // colour paints nothing. RoundedView has a pixel fn.
                            bar := RoundedView {
                                width: 28
                                height: 3
                                draw_bg +: { color: #x5c5c68 radius: 1.5 }
                            }
                        }
                    }
                    prompt_row := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 3
                        padding: Inset{left: 8 right: 8 top: 4 bottom: 4}
                        show_bg: true
                        draw_bg +: { color: #x242429 }
                        prompt_field := TextInput {
                            width: Fill
                            height: 56
                            is_multiline: true
                            empty_text: "what should change\u{2026} Ctrl+Enter sends \u{00b7} Alt+Enter queues"
                            draw_bg +: {
                                color: #x1b1b1b
                                border_radius: 3.0
                            }
                            draw_text +: {
                                color: #xe8e8d0
                                text_style +: { font_size: 8.5 }
                            }
                        }
                        prompt_bar := View {
                            width: Fill
                            height: Fit
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            prompt_status := FabLabelSmall {
                                width: Fill
                                text: ""
                                max_lines: 1
                                text_overflow: TextOverflow.Ellipsis
                                draw_text +: { color: #xffa040 }
                            }
                            queue := Button {
                                width: Fit
                                height: 15
                                padding: Inset{left: 3 right: 5 top: 0 bottom: 0}
                                margin: Inset{left: 0 right: 2 top: 0 bottom: 0}
                                spacing: 3
                                align: Align{x: 0.5 y: 0.5}
                                icon_walk: Walk{width: 9 height: Fit}
                                text: "queue"
                                draw_bg +: { color: #x00000000 }
                                draw_text +: {
                                    color: #xc8c8d4
                                    text_style +: { font_size: 7.5 }
                                }
                                draw_icon +: {
                                    color: #xc8c8d4
                                    svg: crate_resource("self:resources/icons/note_queue.svg")
                                }
                            }
                            send := Button {
                                width: Fit
                                height: 15
                                padding: Inset{left: 3 right: 5 top: 0 bottom: 0}
                                margin: Inset{left: 0 right: 0 top: 0 bottom: 0}
                                spacing: 3
                                align: Align{x: 0.5 y: 0.5}
                                icon_walk: Walk{width: 9 height: Fit}
                                text: "send"
                                draw_bg +: { color: #x00000000 }
                                draw_text +: {
                                    color: #x8fd8ff
                                    text_style +: { font_size: 7.5 }
                                }
                                draw_icon +: {
                                    color: #x8fd8ff
                                    svg: crate_resource("self:resources/icons/note_send.svg")
                                }
                            }
                        }
                    }
                    ident_footer := View {
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 2
                        show_bg: true
                        draw_bg +: { color: #x2b2b30 }
                        divider := View { width: Fill height: 1 margin: Inset{left: 0 top: 0 right: 0 bottom: 4} show_bg: true draw_bg +: { color: #x4a4a52 } }
                        padding: Inset{left: 8 right: 8 top: 0 bottom: 6}
                        scope_row := View {
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 3
                            padding: Inset{left: 0 right: 0 top: 0 bottom: 2}
                            scope_line := View {
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 4
                                align: Align{x: 0.0 y: 0.5}
                                scope_label := FabLabelSmall { width: Fit text: "scope" }
                                scope_this := Button { width: Fit height: 18 padding: Inset{left: 8 right: 8 top: 1 bottom: 1} text: "this" draw_text +: { text_style +: { font_size: 8.0 } } }
                                scope_all := Button { width: Fit height: 18 padding: Inset{left: 8 right: 8 top: 1 bottom: 1} text: "all" draw_text +: { text_style +: { font_size: 8.0 } } }
                                scope_isolated := Button { width: Fit height: 18 padding: Inset{left: 8 right: 8 top: 1 bottom: 1} text: "isolated" draw_text +: { text_style +: { font_size: 8.0 } } }
                            }
                            scope_origin := FabLabelSmall { width: Fill text: "" }
                            // What the buttons MEAN belongs on the buttons,
                            // not on a permanent line under them: it is read
                            // once and then it is just a line taking up the
                            // footer for the rest of the session.
                            scope_tip := Tooltip {
                                width: 0
                                height: 0
                                clip_x: false
                                clip_y: false
                                content := RoundedView {
                                    width: Fit
                                    height: Fit
                                    padding: Inset{left: 8 right: 8 top: 6 bottom: 6}
                                    draw_bg +: {
                                        color: #x2a2a2a
                                        border_size: 1.0
                                        border_color: #x555555
                                        radius: 3.
                                    }
                                    tooltip_label := FabLabelSmall {
                                        width: 230
                                        text: ""
                                    }
                                }
                            }
                        }
                        title_label := FabLabelDim { width: Fill text: "tweak" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                        // The path line is wrapped so the CLICK has a rect
                        // to hit: a Label's own area reports a few points
                        // wide whatever it renders, a View's is the real
                        // one. Clicking it copies the full path.
                        path_row := View {
                            width: Fill
                            height: Fit
                            flow: Down
                            path_label := FabLabelSmall { width: Fill text: "click a widget to inspect it" max_lines: 1 text_overflow: TextOverflow.Ellipsis }
                        }
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        // Make the sidebar part of the widget tree (under this tweaker):
        // /snap lists its rows, so the same remote agent that watches the
        // session can drive the sidebar's fields too.
        cx.widget_tree_insert_child(self.uid, live_id!(sidebar), sidebar.clone());
        self.props_list_uid = sidebar
            .child(live_id!(props_wrap))
            .child(live_id!(props))
            .widget_uid()
            .0;
        self.tree_list_uid = sidebar
            .child(live_id!(tree_wrap))
            .child(live_id!(tree))
            .widget_uid()
            .0;
        let tree_head = sidebar.child(live_id!(tree_wrap)).child(live_id!(tree_head));
        self.tree_isolate_uid = tree_head.child(live_id!(isolate)).widget_uid().0;
        self.view_center_uid = tree_head.child(live_id!(center)).widget_uid().0;
        self.view_zoom_uid = tree_head.child(live_id!(zoom)).widget_uid().0;
        self.shader_list_uid = sidebar
            .child(live_id!(shader_col))
            .child(live_id!(shader_rows))
            .widget_uid()
            .0;
        self.sidebar = Some(sidebar);
    }

    /// The floating extrusion readout: a label and a scrub field, parked in
    /// the app's top-right while the exploded view is up.
    fn ensure_spread_ui(&mut self, cx: &mut Cx) {
        if self.spread_ui.is_some() {
            return;
        }
        let ui = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                // No plate of its own: the readout sits ON the app, and a
                // panel-coloured slab in the corner would read as another
                // piece of sidebar that had come loose. The field keeps its
                // own background — that one is telling you it can be typed
                // in and dragged.
                View {
                    width: 116
                    height: Fit
                    flow: Right
                    spacing: 5
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 6 top: 4 bottom: 4}
                    caption := FabLabelSmall { width: Fit text: "extrude" }
                    value := FabValueInput { width: 48 height: 18 }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        self.spread_uid = ui.child(live_id!(value)).widget_uid().0;
        if let Some(mut field) = ui.child(live_id!(value)).borrow_mut::<FabValueInput>() {
            field.set_hint(
                Some(SPLODED_SPREAD_MIN as f64),
                Some(SPLODED_SPREAD_MAX as f64),
                Some(0.01),
            );
        }
        cx.widget_tree_insert_child(self.uid, live_id!(spread_hud), ui.clone());
        self.spread_ui = Some(ui);
    }

    /// Record a rename the person asked for: `/tweak/state` reports it as a
    /// `rename` alongside the selection, and the log ring carries it, so the
    /// AI can do it in the source where it belongs.
    fn request_rename(&mut self, cx: &mut Cx, to: &str) {
        let Some(sel) = session().lock().unwrap().pinned.clone() else { return };
        let reference = self.sel_ref(cx, sel.uid);
        let from = tree_name_of(cx, sel.uid);
        let renames = {
            let mut s = session().lock().unwrap();
            s.load_renames();
            s.renames.retain(|r| r.reference != reference);
            if !to.is_empty() && to != from {
                s.renames.push(TweakRename {
                    reference: reference.clone(),
                    from: from.clone(),
                    to: to.to_string(),
                });
                s.vibe_status = format!("wants to be called `{to}` \u{2014} the AI renames it");
            }
            s.renames.clone()
        };
        name_store_save(&renames);
        if to.is_empty() || to == from {
            log!("TWEAK rename request dropped for {reference}");
        } else {
            log!("TWEAK rename request {reference} ({}) -> {to}", sel.ty);
        }
        self.redraw_sidebar(cx);
    }

    /// Ask for the selection to become a grid or a flex container. A
    /// widget's type cannot change through an apply, so this is recorded the
    /// way a rename is and the agent edits the source; asking again takes
    /// the request back.
    fn request_convert(&mut self, cx: &mut Cx, to: &str) {
        let Some(sel) = session().lock().unwrap().pinned.clone() else { return };
        let reference = self.sel_ref(cx, sel.uid);
        let (converts, asked) = {
            let mut s = session().lock().unwrap();
            s.load_converts();
            let had = s.converts.iter().any(|c| c.reference == reference);
            s.converts.retain(|c| c.reference != reference);
            if !had {
                s.converts.push(TweakConvert {
                    reference: reference.clone(),
                    from: sel.ty.clone(),
                    to: to.to_string(),
                });
                s.vibe_status = format!("wants to be a {to} \u{2014} the AI changes the type");
            }
            (s.converts.clone(), !had)
        };
        layout_store_save(&converts);
        if asked {
            log!("TWEAK layout request {reference} ({}) -> {to}", sel.ty);
        } else {
            log!("TWEAK layout request taken back for {reference}");
        }
        self.redraw_sidebar(cx);
    }

    /// Push Center / Zoom down onto the view transform.
    fn apply_view_focus(&mut self, cx: &mut Cx) {
        let zoom = self.view_zoom.max(1.0);
        let (center, level) = match self.focus_point(cx) {
            Some((point, level)) => (Some(point), level),
            None => (None, 0.0),
        };
        cx.sploded_set_focus(center, level, zoom);
    }

    /// The layout point the view holds in the middle of the screen, or
    /// `None` when Centre is off (the window centres on itself).
    ///
    /// Centre acts on WHAT IS SELECTED. Isolation, when it is locked onto a
    /// widget, is the stronger statement of "this is the subject" and wins;
    /// otherwise it is the pin, so Centre works on its own without having to
    /// isolate first.
    ///
    /// "The middle of the screen" is the middle of what is left of it — the
    /// panel band covers the right edge — but that correction belongs in
    /// screen pixels, not here: see `SplodedParams::pan`.
    /// The plane is part of the answer: in the exploded view the rotation
    /// displaces a layer across the screen in proportion to its depth, so
    /// centring needs to know WHICH sheet the widget is on, not just where
    /// it sits on that sheet.
    fn focus_point(&self, cx: &Cx) -> Option<(Vec2d, f32)> {
        if !self.view_center {
            return None;
        }
        let uid = if self.tree_isolate && self.isolate_uid != 0 {
            self.isolate_uid
        } else {
            session().lock().unwrap().pinned.as_ref()?.uid
        };
        let widget = cx.widget_tree().widget(WidgetUid(uid));
        if widget.is_empty() {
            return None;
        }
        let rect = widget.area().clipped_rect_union(cx);
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return None;
        }
        let level = cx.sploded_depth_of(uid).unwrap_or(0) as f32;
        Some((
            dvec2(
                rect.pos.x + rect.size.x * 0.5,
                rect.pos.y + rect.size.y * 0.5,
            ),
            level,
        ))
    }

    /// A pin badge was clicked: make its widget the selection and put its
    /// note on screen, open and focused. Unlike the hotkey this never
    /// toggles — clicking a pin means "show me that note".
    fn open_badged_note(&mut self, cx: &mut Cx, uid: u64) {
        let widget = cx.widget_tree().widget(WidgetUid(uid));
        if widget.is_empty() {
            return;
        }
        let rect = widget.area().clipped_rect_union(cx);
        let center = dvec2(rect.pos.x + rect.size.x * 0.5, rect.pos.y + rect.size.y * 0.5);
        let window_id = self.my_window.unwrap_or(0);
        let Some(pick) = pick_of_widget(cx, &widget, center, window_id) else {
            return;
        };
        let path = self.sel_ref(cx, uid);
        log!("TWEAK note on {path}");
        {
            let mut s = session().lock().unwrap();
            s.pinned = Some(pick);
            // The badge callers always have a note already; the right click
            // may be the first thing ever said about this widget, so make
            // one — without it the tab had nothing to show at all.
            s.load_notes();
            if !s.notes.iter().any(|n| n.path == path) {
                s.notes.push(TweakNote::new(path));
            }
        }
        cx.set_key_focus(Area::Empty);
        self.panel_tab = PanelTab::Spec;
        self.rows_uid = 0;
        self.redraw_sidebar(cx);
        self.redraw_overlay(cx);
    }

    /// Open or close the note card on the item we are IN — the pinned
    /// selection, else the widget under the hover (which becomes the
    /// selection, so the tab has something to be about).
    fn toggle_note(&mut self, cx: &mut Cx) {
        let sel_uid = {
            let mut s = session().lock().unwrap();
            if s.pinned.is_none() {
                if let Some(hover) = s.hover.clone() {
                    s.pinned = Some(hover);
                }
            }
            s.pinned.as_ref().map(|p| p.uid)
        };
        let Some(uid) = sel_uid else { return };
        let path = self.sel_ref(cx, uid);
        // There is nothing to open or shut any more: writing about a widget
        // is a tab, so this toggles between that tab and the one before it.
        if self.panel_tab == PanelTab::Spec {
            self.panel_tab = PanelTab::Props;
            self.note_key_shown.clear();
            let mut s = session().lock().unwrap();
            s.notes.retain(|n| !n.text.trim().is_empty() || !n.rules.trim().is_empty());
        } else {
            self.panel_tab = PanelTab::Spec;
            let mut s = session().lock().unwrap();
            s.load_notes();
            if !s.notes.iter().any(|n| n.path == path) {
                s.notes.push(TweakNote::new(path));
            }
        }
        self.redraw_overlay(cx);
    }

    /// Is something being TYPED into? The keys that act on a selection — the
    /// hierarchy arrows, Cmd+Z — belong to the caret whenever there is one,
    /// and to the selection whenever there is not. Asking the panel's own
    /// fields directly is what lets the arrows keep working while the TREE
    /// has focus: a tree row is a selection, not a text cursor, so walking
    /// the hierarchy from it is exactly what the arrows should do.
    fn focus_is_text(&self, cx: &Cx) -> bool {
        if cx.key_focus() == Area::Empty {
            return false;
        }
        let mut fields: Vec<Area> = Vec::new();
        if let Some(sidebar) = self.sidebar.as_ref() {
            fields.push(sidebar.child(live_id!(filter_row)).child(live_id!(search)).area());
            fields.push(sidebar.child(live_id!(prompt_row)).child(live_id!(prompt_field)).area());
            for index in 0..3 {
                if let Some(field) = self.spec_field(index) {
                    fields.push(field.area());
                }
            }
        }
        // The property rows' own inputs come and go with the selection, so
        // ask the live ones rather than keeping a list.
        for row in self.visible.iter() {
            fields.push(row.item.child(live_id!(value)).area());
            fields.push(row.item.child(live_id!(name_field)).area());
        }
        fields.iter().any(|area| !area.is_empty() && cx.has_key_focus(*area))
    }

    /// The selection's exact reference: its indexed path. This is what a note
    /// is keyed by, what the footer copies and what an @mention writes — a
    /// bare `path` renders every unnamed widget as `-` and would key three
    /// different containers to one note.
    fn sel_ref(&mut self, cx: &Cx, uid: u64) -> String {
        if self.sel_ref.0 != uid || self.sel_ref.1.is_empty() {
            self.sel_ref = (uid, indexed_path(cx, uid));
        }
        self.sel_ref.1.clone()
    }

    /// The Spec tab's note key, by the current selection.
    fn note_path(&mut self, cx: &Cx) -> Option<String> {
        if self.panel_tab != PanelTab::Spec {
            return None;
        }
        let uid = session().lock().unwrap().pinned.as_ref().map(|p| p.uid)?;
        Some(self.sel_ref(cx, uid))
    }

    /// Take the card's live text into the session (the TextInput only
    /// reports on commit, and a send must carry what is on screen).

    /// The Spec tab: what is written ABOUT the selection, and the rules
    /// that stand over the whole app.
    ///
    /// Each field is read back into the model while it HAS the caret and
    /// seeded from the model while it does not. That asymmetry is the whole
    /// trick: seeding unconditionally overwrites what is being typed every
    /// frame, and reading unconditionally lets a stale field overwrite the
    /// model when the selection changes under it.
    fn draw_spec(&mut self, cx: &mut Cx2d, sel: Option<&TweakPick>) {
        let Some(sidebar) = self.sidebar.clone() else { return };
        let col = sidebar.child(live_id!(spec_col));
        let path = sel.map(|p| self.sel_ref(cx, p.uid));
        let path_for_seed = path.clone();

        col.child(live_id!(spec_for)).set_text(
            cx,
            &match (&path, sel) {
                (Some(path), Some(sel)) => format!("{}  \u{2022}  {}", sel.ty, tail_ellipsis(path, 40)),
                _ => "nothing selected \u{2014} the app rules below still work".to_string(),
            },
        );

        // notes and rules belong to the selection; with nothing selected
        // there is nothing for them to be about, so they say so and stay
        // out of the way rather than writing to a record that has no path.
        // One View carries all four, because only a View's visibility can be
        // toggled: `TextInput` has no `visible` of its own, so `set_visible`
        // on the fields themselves was silently nothing -- the headers hid
        // (Label declares one) and the boxes stayed.
        let have = path.is_some();
        for index in 0..2 {
            if let Some((head, bx, _)) = self.spec_row(index) {
                head.set_visible(cx, have);
                bx.set_visible(cx, have);
            }
        }
        self.note_text_uid = self.spec_field(0).map(|f| f.widget_uid().0).unwrap_or(0);
        // The dragged split, in case it changed while the tab was not up.
        self.spec_apply_weights(cx);
        // The splitters only exist while there is something above them to
        // split with: with nothing selected the two upper rows are gone and
        // the app rules header is just a header.
        for index in 1..3 {
            if let Some((head, _, _)) = self.spec_row(index) {
                head.child(live_id!(grip)).set_visible(cx, have);
            }
        }
        {
            // The cross is only there when there is something to clear: an
            // always-present one invites a click that does nothing, and this
            // is the only control in the tab that destroys what you wrote.
            if let Some((head, _, field)) = self.spec_row(0) {
                let clear = head.child(live_id!(notes_clear));
                self.spec_clear_uid = clear.widget_uid().0;
                clear.set_visible(cx, !field.text().is_empty());
            }
        }

        if let Some(path) = path_for_seed {
            let (mut notes, mut rules) = {
                let mut s = session().lock().unwrap();
                s.load_notes();
                match s.notes.iter().find(|n| n.path == path) {
                    Some(note) => (note.text.clone(), note.rules.clone()),
                    None => (String::new(), String::new()),
                }
            };
            // `note_key_shown` is the path the two fields were last SEEDED
            // for, and it is written here and nowhere else. The frame the
            // selection moves, the fields still hold the old widget's text
            // -- and if that frame read them back, it would commit the old
            // widget's words to the new widget's record, destroying whatever
            // the new one had. So a changed path seeds, unconditionally,
            // caret or no caret; only a field seeded for THIS path is ever
            // read back into it.
            let seeded = self.note_key_shown == path;
            let mut changed = false;
            for (index, slot) in [(0usize, &mut notes), (1usize, &mut rules)] {
                let Some(field) = self.spec_field(index) else { continue };
                let focused = field.area() != Area::Empty && cx.has_key_focus(field.area());
                if seeded && focused {
                    let typed = field.text();
                    if typed != *slot {
                        *slot = typed;
                        changed = true;
                    }
                } else if field.text() != *slot {
                    field.set_text(cx, slot);
                }
            }
            self.note_key_shown = path.clone();
            if changed {
                let mut s = session().lock().unwrap();
                if !s.notes.iter().any(|n| n.path == path) {
                    s.notes.push(TweakNote::new(path.clone()));
                }
                if let Some(note) = s.notes.iter_mut().find(|n| n.path == path) {
                    note.text = notes;
                    note.rules = rules;
                }
                drop(s);
                self.spec_dirty = true;
            }
        }

        if path.is_none() {
            self.note_key_shown.clear();
        }

        // The app rules need no selection -- that is the reason this tab
        // has to draw with nothing picked at all.
        {
            let Some(field) = self.spec_field(2) else { return };
            let live = {
                let mut s = session().lock().unwrap();
                s.load_app_rules();
                s.app_rules.clone()
            };
            if field.area() != Area::Empty && cx.has_key_focus(field.area()) {
                let typed = field.text();
                if typed != live {
                    session().lock().unwrap().app_rules = typed;
                    self.spec_dirty = true;
                }
            } else if field.text() != live {
                field.set_text(cx, &live);
            }
        }

        // Nothing in the tab has the caret any more, so what was typed is
        // finished: put it on disk.
        if self.spec_dirty
            && !(0..3).any(|index| {
                let Some(field) = self.spec_field(index) else { return false };
                let area = field.area();
                !area.is_empty() && cx.has_key_focus(area)
            })
        {
            self.spec_flush();
        }
    }

    /// A one-line explanation for the control under the pointer, if it is
    /// one of the panel's own. The property rows already explain themselves
    /// through `row_docs`; this covers everything else a person can press --
    /// the tabs, the filter row, the tree head, the scope buttons, the Spec
    /// tab's cross and splitters, the strip's buttons, the shader fold, the
    /// footer's path line, the extrusion readout, and the segments inside a
    /// hovered row (flow, align, size mode, the box legs).
    ///
    /// Shown through the same Tooltip the scope buttons use -- see
    /// `chrome_tip_hover` -- so every control in the panel explains itself
    /// in one voice.
    fn chrome_doc(&self, cx: &Cx, abs: Vec2d) -> Option<(Rect, String)> {
        fn place(rect: Rect, text: &str) -> (Rect, String) {
            (rect, text.to_string())
        }
        fn hit(cx: &Cx, w: &WidgetRef, abs: Vec2d) -> Option<Rect> {
            let r = w.area().rect(cx);
            (r.size.x > 0.0 && r.size.y > 0.0 && r.contains(abs)).then_some(r)
        }

        // The extrusion readout floats over the app, outside the band.
        if let Some(ui) = self.spread_ui.as_ref() {
            if let Some(r) = hit(cx, &ui.child(live_id!(value)), abs) {
                return Some(place(r, "how far the exploded layers stand apart \u{00b7} drag to scrub, wheel to extrude"));
            }
        }
        let sidebar = self.sidebar.as_ref()?;
        if self.band.size.x <= 0.0 || abs.x < self.band.pos.x {
            return None;
        }

        // A hovered property row's own segments first: those ids repeat in
        // every row, so they are only meaningful inside the row under the
        // pointer.
        if let Some(row) = self
            .visible
            .iter()
            .find(|row| {
                let r = row.item.area().clipped_rect(cx);
                r.size.y > 0.0 && r.contains(abs)
            })
        {
            let item = &row.item;
            let inner: [(&[LiveId], &str); 63] = [
                (&[live_id!(flow_col), live_id!(dir_row), live_id!(flow_seg), live_id!(f_right)], "children flow left to right"),
                (&[live_id!(flow_col), live_id!(dir_row), live_id!(flow_seg), live_id!(f_down)], "children flow top to bottom"),
                (&[live_id!(flow_col), live_id!(dir_row), live_id!(flow_seg), live_id!(f_over)], "children stack on top of each other"),
                (&[live_id!(flow_col), live_id!(dir_row), live_id!(f_wrap)], "children that run out of width start a new row"),
                (&[live_id!(flow_col), live_id!(dir_row), live_id!(ra_seg), live_id!(ra_top)], "the children of a row line up along its top"),
                (&[live_id!(flow_col), live_id!(dir_row), live_id!(ra_seg), live_id!(ra_mid)], "the children of a row line up on its centre line"),
                (&[live_id!(flow_col), live_id!(dir_row), live_id!(ra_seg), live_id!(ra_bottom)], "the children of a row sit on its baseline"),
                (&[live_id!(flow_col), live_id!(gap_row), live_id!(spacing_input)], "space between the children, in points"),
                (&[live_id!(flow_col), live_id!(gap_row), live_id!(wrap_box), live_id!(wrap_input)], "space between wrapped rows, in points"),
                (&[live_id!(align_col), live_id!(just_row), live_id!(just_seg), live_id!(j_start)], "children gather at the start of the flow"),
                (&[live_id!(align_col), live_id!(just_row), live_id!(just_seg), live_id!(j_mid)], "children gather in the middle of the flow"),
                (&[live_id!(align_col), live_id!(just_row), live_id!(just_seg), live_id!(j_end)], "children gather at the end of the flow"),
                (&[live_id!(align_col), live_id!(space_row), live_id!(space_seg), live_id!(s_between)], "the free space goes between the children"),
                (&[live_id!(align_col), live_id!(space_row), live_id!(space_seg), live_id!(s_around)], "each child gets an equal share of free space on both sides"),
                (&[live_id!(align_col), live_id!(space_row), live_id!(space_seg), live_id!(s_evenly)], "every gap, edges included, gets the same free space"),
                (&[live_id!(align_col), live_id!(cross_row), live_id!(cross_seg), live_id!(c_start)], "across the flow, children sit at the start"),
                (&[live_id!(align_col), live_id!(cross_row), live_id!(cross_seg), live_id!(c_mid)], "across the flow, children sit in the middle"),
                (&[live_id!(align_col), live_id!(cross_row), live_id!(cross_seg), live_id!(c_end)], "across the flow, children sit at the end"),
                (&[live_id!(ctr_col), live_id!(mode_row), live_id!(convert)], "ask the agent to change this container's type in the source \u{00b7} press again to take it back"),
                (&[live_id!(ctr_col), live_id!(name_row), live_id!(ctr_name)], "name this container so its children can size in cqw / cqh of it"),
                (&[live_id!(abs_col), live_id!(abs_check)], "take it out of the flow and place it at x, y in its parent"),
                (&[live_id!(abs_col), live_id!(abs_xy), live_id!(abs_x)], "points from the parent's left"),
                (&[live_id!(abs_col), live_id!(abs_xy), live_id!(abs_y)], "points from the parent's top"),
                (&[live_id!(align_col), live_id!(just_row), live_id!(just_seg), live_id!(j_stretch)], "each cell's child stretches across its column"),
                (&[live_id!(align_col), live_id!(cross_row), live_id!(cross_seg), live_id!(c_stretch)], "each cell's child stretches down its row"),
                (&[live_id!(grid_col), live_id!(cols_row), live_id!(cols_in)], "the columns, as CSS \u{00b7} 70px 20% 1fr minmax(60px, 1fr) repeat(2, minmax(50px, 1fr))"),
                (&[live_id!(grid_col), live_id!(rows_row), live_id!(rows_in)], "the rows, as CSS \u{00b7} 48px 1fr minmax(40px, auto-fit)"),
                (&[live_id!(grid_col), live_id!(gaps_row), live_id!(gap_col)], "space between columns, in points"),
                (&[live_id!(grid_col), live_id!(gaps_row), live_id!(gap_row_in)], "space between rows, in points"),
                (&[live_id!(grid_col), live_id!(fill_row), live_id!(fill_seg), live_id!(f_rows)], "children without a place fill each row before the next"),
                (&[live_id!(grid_col), live_id!(fill_row), live_id!(fill_seg), live_id!(f_cols)], "children without a place fill each column before the next"),
                (&[live_id!(grid_col), live_id!(areas_row), live_id!(areas_in)], "named areas: one row per / and a . for an empty cell \u{00b7} hero hero . / . . ."),
                (&[live_id!(cell_col), live_id!(place_row), live_id!(cell_c)], "the column it starts in, from 1 \u{00b7} 0 lets the fill order place it"),
                (&[live_id!(cell_col), live_id!(place_row), live_id!(cell_r)], "the row it starts in, from 1 \u{00b7} 0 lets the fill order place it"),
                (&[live_id!(cell_col), live_id!(span_row), live_id!(cell_cs)], "how many columns it spans \u{00b7} 0 for one"),
                (&[live_id!(cell_col), live_id!(span_row), live_id!(cell_rs)], "how many rows it spans \u{00b7} 0 for one"),
                (&[live_id!(cell_col), live_id!(area_row), live_id!(cell_area)], "the named area it fills, from the grid's areas"),
                (&[live_id!(link)], "one value for all four sides"),
                (&[live_id!(size_col), live_id!(w_row), live_id!(w_seg), live_id!(w_fill)], "width: fill whatever the parent leaves"),
                (&[live_id!(size_col), live_id!(w_row), live_id!(w_seg), live_id!(w_fit)], "width: fit the content"),
                (&[live_id!(size_col), live_id!(w_row), live_id!(w_seg), live_id!(w_fix)], "width: a fixed size, in points"),
                (&[live_id!(size_col), live_id!(w_row), live_id!(w_input)], "the width \u{00b7} a number, or a size such as 50% or calc(100% - 20px)"),
                (&[live_id!(size_col), live_id!(h_row), live_id!(h_seg), live_id!(h_fill)], "height: fill whatever the parent leaves"),
                (&[live_id!(size_col), live_id!(h_row), live_id!(h_seg), live_id!(h_fit)], "height: fit the content"),
                (&[live_id!(size_col), live_id!(h_row), live_id!(h_seg), live_id!(h_fix)], "height: a fixed size, in points"),
                (&[live_id!(size_col), live_id!(h_row), live_id!(h_input)], "the height \u{00b7} a number, or a size such as 50% or calc(100% - 20px)"),
                (&[live_id!(size_col), live_id!(w_clamp), live_id!(w_min)], "never narrower than this \u{00b7} empty for no bound"),
                (&[live_id!(size_col), live_id!(w_clamp), live_id!(w_max)], "never wider than this \u{00b7} empty for no bound"),
                (&[live_id!(size_col), live_id!(h_clamp), live_id!(h_min)], "never shorter than this \u{00b7} empty for no bound"),
                (&[live_id!(size_col), live_id!(h_clamp), live_id!(h_max)], "never taller than this \u{00b7} empty for no bound"),
                (&[live_id!(size_col), live_id!(w_grow), live_id!(w_weight)], "its share of the free width against its siblings"),
                (&[live_id!(size_col), live_id!(w_grow), live_id!(w_shrink)], "how readily it gives up width when there is too little \u{00b7} 0 never"),
                (&[live_id!(size_col), live_id!(w_basis), live_id!(w_basis_in)], "the width it starts from before the free space is shared"),
                (&[live_id!(size_col), live_id!(h_grow), live_id!(h_weight)], "its share of the free height against its siblings"),
                (&[live_id!(size_col), live_id!(h_grow), live_id!(h_shrink)], "how readily it gives up height when there is too little \u{00b7} 0 never"),
                (&[live_id!(size_col), live_id!(h_basis), live_id!(h_basis_in)], "the height it starts from before the free space is shared"),
                (&[live_id!(size_col), live_id!(aspect_row), live_id!(aspect_in)], "width over height \u{00b7} 1.5 or 3:2 \u{00b7} empty for none"),
                (&[live_id!(box_col), live_id!(top_row), live_id!(leg_top)], "the top side, in points"),
                (&[live_id!(box_col), live_id!(mid_row), live_id!(leg_left)], "the left side, in points"),
                (&[live_id!(box_col), live_id!(mid_row), live_id!(leg_right)], "the right side, in points"),
                (&[live_id!(box_col), live_id!(bot_row), live_id!(leg_bottom)], "the bottom side, in points"),
                (&[live_id!(tname_wrap), live_id!(tname)], "ask the agent to give this widget a name in the source"),
                (&[live_id!(value)], "the value \u{00b7} drag to scrub, double-click the label to reset"),
            ];
            for (path, text) in inner {
                let mut w = item.clone();
                for id in path {
                    w = w.child(*id);
                }
                if let Some(r) = hit(cx, &w, abs) {
                    return Some(place(r, text));
                }
            }
        }

        let chrome: [(&[LiveId], &str); 23] = [
            (&[live_id!(filter_row), live_id!(search)], "filter the properties by name"),
            (&[live_id!(filter_row), live_id!(select)], "hand the mouse back to the app: its buttons work, the selection stays"),
            (&[live_id!(filter_row), live_id!(sploded)], "explode the widget tree into layers \u{00b7} wheel extrudes, drag orbits"),
            (&[live_id!(tab_row), live_id!(tab_props)], "the selection's properties, edited live"),
            (&[live_id!(tab_row), live_id!(tab_shader)], "the selection's draw layers: preview, source, states"),
            (&[live_id!(tab_row), live_id!(tab_tree)], "the widget tree: isolate a branch, centre, zoom"),
            (&[live_id!(tab_row), live_id!(tab_theme)], "the theme's colours and values, edited live everywhere"),
            (&[live_id!(tab_row), live_id!(tab_spec)], "notes and rules about the selection, and rules for the whole app"),
            (&[live_id!(tree_wrap), live_id!(tree_head), live_id!(isolate)], "show only the selection and what is inside it"),
            (&[live_id!(tree_wrap), live_id!(tree_head), live_id!(center)], "keep the view centred on the selection"),
            (&[live_id!(tree_wrap), live_id!(tree_head), live_id!(zoom)], "magnify the app view \u{00b7} 1 is life size"),
            (&[live_id!(shader_col), live_id!(src_fold)], "show the shader's source \u{00b7} Ctrl+Enter applies an edit"),
            (&[live_id!(shader_col), live_id!(states_row), live_id!(states_pause)], "pause the animated state previews"),
            (&[live_id!(spec_col), live_id!(notes_head), live_id!(notes_clear)], "empty the notes"),
            (&[live_id!(spec_col), live_id!(rules_head), live_id!(grip)], "drag to share the tab's height between the fields"),
            (&[live_id!(spec_col), live_id!(app_head), live_id!(grip)], "drag to share the tab's height between the fields"),
            (&[live_id!(prompt_row), live_id!(prompt_field)], "tell the agent what should change about the selection"),
            (&[live_id!(prompt_row), live_id!(prompt_bar), live_id!(queue)], "hold this message for the next send (Alt+Enter)"),
            (&[live_id!(prompt_row), live_id!(prompt_bar), live_id!(send)], "send to the agent now (Ctrl+Enter)"),
            (&[live_id!(ident_footer), live_id!(scope_row), live_id!(scope_line), live_id!(scope_this)], "edits change this instance only"),
            (&[live_id!(ident_footer), live_id!(scope_row), live_id!(scope_line), live_id!(scope_all)], "edits change the type, so every widget of this type"),
            (&[live_id!(ident_footer), live_id!(scope_row), live_id!(scope_line), live_id!(scope_isolated)], "keep 'all' inside the isolated branch"),
            (&[live_id!(ident_footer), live_id!(path_row), live_id!(path_label)], "the selection's address \u{00b7} click copies it"),
        ];
        for (path, text) in chrome {
            let mut w = sidebar.clone();
            for id in path {
                w = w.child(*id);
            }
            if let Some(r) = hit(cx, &w, abs) {
                return Some(place(r, text));
            }
        }
        None
    }

    /// The Spec tab's rows by index -- 0 notes, 1 rules, 2 app rules -- as
    /// (header, box, field). The header carries the label and, for the two
    /// lower rows, the splitter; the box is the flex row that shares the
    /// tab's height; the field is the TextInput inside it.
    fn spec_row(&self, index: usize) -> Option<(WidgetRef, WidgetRef, WidgetRef)> {
        let col = self.sidebar.as_ref()?.child(live_id!(spec_col));
        let (head, bx, field) = match index {
            0 => (live_id!(notes_head), live_id!(notes_box), live_id!(spec_notes)),
            1 => (live_id!(rules_head), live_id!(rules_box), live_id!(spec_rules)),
            _ => (live_id!(app_head), live_id!(app_box), live_id!(spec_app)),
        };
        let bx = col.child(bx);
        Some((col.child(head), bx.clone(), bx.child(field)))
    }

    /// Push the session's weights at the three boxes, whichever have
    /// changed since last time -- see `spec_applied_w`. Called from the
    /// draw, and from the drag itself, so a move lands on the very next
    /// layout pass instead of the one after.
    ///
    /// Set through the typed walk, not the script: the apply macro
    /// evaluates without the markup's prelude, so `Fill` is not in scope
    /// there, and a split set that way never landed at all.
    fn spec_apply_weights(&mut self, cx: &mut Cx) {
        for index in 0..3 {
            let w = spec_weight(index);
            if (self.spec_applied_w[index] - w).abs() <= 0.01 {
                continue;
            }
            self.spec_applied_w[index] = w;
            let Some((_, bx, _)) = self.spec_row(index) else { continue };
            // A named guard, so it is dropped before `bx` rather than after
            // it: an if-let's temporary lives to the end of the statement,
            // which here is the end of the loop body, past the local it
            // borrows.
            let guard = bx.borrow_mut::<crate::View>();
            if let Some(mut view) = guard {
                view.walk.height = Size::Fill {
                    weight: w,
                    basis: crate::makepad_draw::FitBound::Abs(0.0),
                    shrink: 0.0,
                    min: Some(SPEC_FIELD_MIN),
                    max: None,
                };
                view.redraw(cx);
            }
        }
    }

    fn spec_field(&self, index: usize) -> Option<WidgetRef> {
        self.spec_row(index).map(|(_, _, field)| field)
    }

    /// Which splitter is under this point, if any: 0 is the boundary
    /// between notes and rules, 1 between rules and app rules. A splitter IS
    /// the header row of the lower field -- a row that was already there,
    /// costing the tab no height of its own -- and it only answers while
    /// the field above it is on screen: with nothing selected the app rules
    /// header has nothing above it to split with.
    ///
    /// Read at event time, so it answers for the frame already on screen,
    /// the rule the property rows follow.
    fn spec_grip_hit(&self, cx: &Cx, abs: Vec2d) -> Option<usize> {
        if self.panel_tab != PanelTab::Spec {
            return None;
        }
        (0..2).find(|&k| {
            let Some((_, above, _)) = self.spec_row(k) else { return false };
            let Some((head, _, _)) = self.spec_row(k + 1) else { return false };
            if above.area().rect(cx).size.y <= 0.0 {
                return false;
            }
            let r = head.area().rect(cx);
            r.size.y > 0.0
                && abs.x >= r.pos.x
                && abs.x <= r.pos.x + r.size.x
                && abs.y >= r.pos.y - 2.0
                && abs.y <= r.pos.y + r.size.y + 2.0
        })
    }

    /// Empty the notes field, and the record behind it. Written through
    /// straight away rather than left for the caret to leave: a clear is a
    /// decision, and there is nothing half-typed to protect.
    fn spec_clear_notes(&mut self, cx: &mut Cx) {
        let Some(sidebar) = self.sidebar.clone() else { return };
        sidebar
            .child(live_id!(spec_col))
            .child(live_id!(notes_box))
            .child(live_id!(spec_notes))
            .set_text(cx, "");
        let path = self.note_key_shown.clone();
        if !path.is_empty() {
            let mut s = session().lock().unwrap();
            if let Some(note) = s.notes.iter_mut().find(|n| n.path == path) {
                note.text.clear();
            }
        }
        self.spec_flush();
        self.redraw_sidebar(cx);
    }

    /// Put what the Spec tab holds on disk: the per-widget records in the
    /// note store, the app-wide document in its own file.
    fn spec_flush(&mut self) {
        self.spec_dirty = false;
        let (notes, rules) = {
            let s = session().lock().unwrap();
            (s.notes.clone(), s.app_rules.clone())
        };
        note_store_save(&notes);
        app_rules_save(&rules);
    }

    /// Is the caret in the prompt strip's box? Every one of the strip's
    /// keys is claimed only there -- Ctrl+Enter belongs to the shader
    /// prompt when the caret is in THAT, and to nothing at all when the
    /// caret is in a property field.
    fn prompt_field_focused(&self, cx: &Cx) -> bool {
        let Some(sidebar) = self.sidebar.as_ref() else { return false };
        let area = sidebar.child(live_id!(prompt_row)).child(live_id!(prompt_field)).area();
        !area.is_empty() && cx.has_key_focus(area)
    }

    /// Nothing typed yet -- the only state in which Up walks the history
    /// instead of moving the caret.
    fn prompt_field_empty(&self) -> bool {
        let Some(sidebar) = self.sidebar.as_ref() else { return false };
        sidebar.child(live_id!(prompt_row)).child(live_id!(prompt_field)).text().is_empty()
    }

    /// Which widget a send is ABOUT. One box serves every widget, so the
    /// attribution is read at the moment of sending rather than carried by
    /// the box. With nothing selected the ask is about the app itself, which
    /// is a real thing to say and must not be dropped on the floor.
    fn prompt_target(&mut self, cx: &Cx) -> String {
        let uid = session().lock().unwrap().pinned.as_ref().map(|p| p.uid);
        match uid {
            Some(uid) => self.sel_ref(cx, uid),
            None => "app".to_string(),
        }
    }

    /// The strip's field into the session. The TextInput is the truth while
    /// the caret is in it; the session is the truth across redraws.
    fn prompt_sync_text(&mut self, cx: &mut Cx) {
        let Some(sidebar) = self.sidebar.clone() else { return };
        let field = sidebar.child(live_id!(prompt_row)).child(live_id!(prompt_field));
        if field.area() == Area::Empty {
            return;
        }
        let text = field.text();
        let mut s = session().lock().unwrap();
        if s.prompt != text {
            s.prompt = text;
        }
        let _ = cx;
    }

    /// Take what is written into the outbox and EMPTY the box, the way a
    /// message box empties when you press send. The text is not lost: it
    /// goes into the recall history, where Up brings it back.
    ///
    /// Returns false when there was nothing written.
    fn prompt_take_draft(&mut self, cx: &mut Cx) -> bool {
        let path = self.prompt_target(cx);
        let text = {
            let mut s = session().lock().unwrap();
            let text = s.prompt.trim().to_string();
            if text.is_empty() {
                return false;
            }
            s.prompt_history.push(text.clone());
            s.prompt_at = s.prompt_history.len();
            s.prompt.clear();
            text
        };
        // From the Shader tab the message is about the layer that tab is
        // showing, and the agent needs the fn sources to rewrite it: both
        // ride with the message, so a queued shader ask still says which
        // layer it meant after the tab has moved on.
        let shader = (self.panel_tab == PanelTab::Shader).then(|| {
            let layer = self.vibe_layer.clone().unwrap_or_else(|| "draw_bg".to_string());
            let fns = self
                .vibe_fn_sources
                .iter()
                .map(|(name, loc, src)| format!("// {name} \u{2014} {loc}\n{src}"))
                .collect::<Vec<_>>()
                .join("\n\n");
            (layer, fns)
        });
        {
            // The ask is counted on the record for the widget it is about,
            // so make one if this is the first thing ever said about it.
            let mut s = session().lock().unwrap();
            s.load_notes();
            if !s.notes.iter().any(|n| n.path == path) {
                s.notes.push(TweakNote::new(path.clone()));
            }
            s.outbox.push(Outgoing { path, text, shader });
        }
        self.prompt_clear_field = true;
        true
    }

    /// Ctrl+Enter: hand the queue to the AI. Same channel the card used --
    /// `/tweak/state` carries the ask and a `TWEAK ask` line lands in the
    /// log ring -- but from one box under the tabs instead of a card.
    fn prompt_send(&mut self, cx: &mut Cx) {
        self.prompt_sync_text(cx);
        self.prompt_take_draft(cx);
        let sent: Vec<Outgoing> = {
            let mut s = session().lock().unwrap();
            std::mem::take(&mut s.outbox)
        };
        if sent.is_empty() {
            session().lock().unwrap().prompt_status =
                "nothing to send: the box is empty".to_string();
        } else {
            for item in &sent {
                let (note_path, text) = (&item.path, &item.text);
                if let Some((layer, fns)) = &item.shader {
                    // A shader ask: the execute bundle on the AI's ear (the
                    // log and /tweak/state carry it), scoped to exactly this
                    // draw layer. Code only -- colours and sizes are the
                    // Props rows' business.
                    log!("TWEAK vibe sel={note_path} layer={layer} prompt={text}");
                    let mut s = session().lock().unwrap();
                    s.vibe_status = format!("sent to the AI \u{00b7} waiting\u{2026} \u{2014} {text}");
                    s.vibe_pending = Some((note_path.clone(), layer.clone()));
                    s.vibes.push((note_path.clone(), layer.clone(), text.clone(), fns.clone()));
                    continue;
                }
                let seq = {
                    let mut s = session().lock().unwrap();
                    match s.notes.iter_mut().find(|n| n.path == *note_path) {
                        Some(note) => {
                            note.sent += 1;
                            note.sent
                        }
                        None => 0,
                    }
                };
                log!("TWEAK ask #{seq} {note_path}: {text}");
            }
            session().lock().unwrap().prompt_status = if sent.len() == 1 {
                "sent to the AI".to_string()
            } else {
                format!("{} messages sent to the AI", sent.len())
            };
        }
        self.prompt_focus_pending = true;
        self.redraw_sidebar(cx);
    }

    /// Alt+Enter: put this message in the queue and wake nobody. It goes out
    /// with the next Ctrl+Enter -- deliberately NOT logged as an ask,
    /// because an ask is what an agent watching the log acts on.
    fn prompt_queue(&mut self, cx: &mut Cx) {
        self.prompt_sync_text(cx);
        if !self.prompt_take_draft(cx) {
            session().lock().unwrap().prompt_status =
                "nothing to queue: the box is empty".to_string();
            self.redraw_sidebar(cx);
            return;
        }
        let count = session().lock().unwrap().outbox.len();
        session().lock().unwrap().prompt_status = match count {
            1 => "1 queued \u{00b7} Ctrl+Enter sends the queue".to_string(),
            n => format!("{n} queued \u{00b7} Ctrl+Enter sends the queue"),
        };
        log!("TWEAK note queued \u{00b7} {count} waiting");
        self.prompt_focus_pending = true;
        self.redraw_sidebar(cx);
    }

    /// Up / Down in an EMPTY box walks the history, the way a shell prompt
    /// does -- so a message just sent is one keypress from being sent again,
    /// or edited and sent again. Only while empty, or Up would be fighting
    /// the caret in a message being written.
    fn prompt_recall(&mut self, cx: &mut Cx, back: bool) {
        let text = {
            let mut s = session().lock().unwrap();
            if s.prompt_history.is_empty() {
                return;
            }
            let len = s.prompt_history.len();
            if back {
                s.prompt_at = s.prompt_at.saturating_sub(1);
            } else if s.prompt_at < len {
                s.prompt_at += 1;
            }
            let at = s.prompt_at;
            let text = s.prompt_history.get(at).cloned().unwrap_or_default();
            s.prompt = text.clone();
            text
        };
        if let Some(sidebar) = self.sidebar.clone() {
            let field = sidebar.child(live_id!(prompt_row)).child(live_id!(prompt_field));
            field.set_text(cx, &text);
        }
        self.redraw_sidebar(cx);
    }

    /// Leaving the tab: put what was typed on disk, keeping the text.
    fn note_close(&mut self, cx: &mut Cx) {
        self.spec_flush();
        // A record opened and left without a word written in it is not a
        // note. Dropping the empties keeps /tweak/state a list of things the
        // person actually said, rather than everywhere they pressed Insert.
        {
            let mut s = session().lock().unwrap();
            s.notes.retain(|n| !n.text.trim().is_empty() || !n.rules.trim().is_empty());
        }
        self.note_key_shown.clear();
        session().lock().unwrap().mention = false;
        // Nothing is being typed into any more, so the arrows go back to
        // walking the hierarchy.
        cx.set_key_focus(Area::Empty);
        self.redraw_overlay(cx);
    }

    /// Walk the live widget tree with the arrow keys, the way a scene
    /// editor does: up to the parent, down to the first child, left/right to
    /// the previous/next sibling. Only ever reached with something selected
    /// — with nothing selected the arrows still orbit the exploded view.
    fn walk_selection(&mut self, cx: &mut Cx, dir: KeyCode) {
        let Some(sel) = session().lock().unwrap().pinned.clone() else {
            return;
        };
        let rows = cx.widget_tree().flat_tree(cx);
        let Some(at) = rows.iter().position(|row| row.uid == sel.uid) else {
            return;
        };
        let depth = rows[at].depth;
        // flat_tree is depth-first: the parent is the nearest earlier row one
        // level up, the siblings are the same-depth rows that share it, and
        // the first child is the very next row when it is one level deeper.
        let parent = rows[..at].iter().rposition(|row| row.depth + 1 == depth);
        let sibling = |step: isize| -> Option<usize> {
            let mut index = at as isize;
            loop {
                index += step;
                if index < 0 || index as usize >= rows.len() {
                    return None;
                }
                let row = &rows[index as usize];
                if row.depth < depth {
                    return None; // left the parent: no sibling that way
                }
                if row.depth == depth {
                    return Some(index as usize);
                }
            }
        };
        let target = match dir {
            KeyCode::ArrowUp => parent,
            KeyCode::ArrowDown => rows
                .get(at + 1)
                .filter(|row| row.depth == depth + 1)
                .map(|_| at + 1),
            KeyCode::ArrowLeft => sibling(-1),
            KeyCode::ArrowRight => sibling(1),
            _ => None,
        };
        let Some(target) = target else {
            log!("TWEAK walk {dir:?}: nothing that way from {}", sel.path);
            return;
        };
        let uid = rows[target].uid;
        let widget = cx.widget_tree().widget(WidgetUid(uid));
        if widget.is_empty() {
            return;
        }
        // Walking into a widget on an unselected tab or inside a closed fold
        // has to OPEN it first, exactly as clicking its tree row does —
        // otherwise the arrows stop at the edge of whatever happens to be
        // showing, which is not the hierarchy.
        reveal_widget(cx, uid);
        let center = {
            let rect = widget.area().clipped_rect_union(cx);
            dvec2(rect.pos.x + rect.size.x * 0.5, rect.pos.y + rect.size.y * 0.5)
        };
        // A widget revealed a moment ago has not been drawn yet, so it has no
        // rect to pick from. Pin it anyway with what is known: the overlay
        // re-reads live rects every frame and the outline lands as soon as it
        // draws.
        let pick = pick_of_widget(cx, &widget, center, sel.window_id).unwrap_or_else(|| {
            let path = cx
                .widget_tree()
                .path_to(WidgetUid(uid))
                .iter()
                .map(|id| live_id_token(*id))
                .collect::<Vec<_>>()
                .join(".");
            TweakPick {
                uid,
                path,
                ty: rows[target].ty.clone(),
                rect: Rect::default(),
                window_id: sel.window_id,
                band: None,
                level: 0,
            }
        });
        log!("TWEAK walk {dir:?} \u{2192} {} ({})", pick.path, pick.ty);
        session().lock().unwrap().pinned = Some(pick);
        self.rows_uid = 0;
        self.redraw_sidebar(cx);
        self.redraw_overlay(cx);
    }

    /// Rebuild the row bindings from the selection's reflected properties:
    /// classify into sections, order each section (box-model progression for
    /// layout, surface-then-content with colors leading for style), and mark
    /// what differs from its session-original (resettable).
    /// The Theme tab's rows: every theme colour (Colours), number
    /// (Spacing & sizes) and font size, each editable in place.
    fn rebuild_theme_rows(&mut self, cx: &mut Cx) {
        self.rows.clear();
        self.cascade.clear();
        self.doc_row = None;
        self.radius_prop = None;
        let (diff, overrides): (Vec<TweakDiffEntry>, Vec<String>) = {
            let s = session().lock().unwrap();
            (
                s.diff.iter().filter(|e| e.scope == "theme").cloned().collect(),
                s.theme_overrides.iter().map(|(n, _)| n.clone()).collect(),
            )
        };
        let mut site = String::new();
        let mut values = theme_values(cx);
        values.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, _, value, loc) in values {
            if site.is_empty() {
                site = loc.clone();
            }
            let (kind, text, section) = match value {
                ThemeVal::Color(c) => (RowKind::Color, hex_of(c), SectionKind::Style),
                ThemeVal::Num(f) => (
                    RowKind::Num,
                    fmt_f64(f),
                    if name.starts_with("font") { SectionKind::Text } else { SectionKind::Layout },
                ),
            };
            let original = diff.iter().find(|e| e.prop == name).map(|e| e.old.clone());
            let changed = overrides.iter().any(|n| *n == name) || original.as_ref().is_some_and(|o| *o != text);
            self.row_docs.insert(name.clone(), format!("theme value, defined in {loc}"));
            self.rows.push(RowBinding {
                prop: name,
                kind,
                value: text,
                quoted: false,
                section,
                set: true,
                changed,
                original,
                field_uid: 0,
                swatch_uid: 0,
                struct_kind: StructKind::None,
                comp_vals: Vec::new(),
                comp_uids: Vec::new(),
                alt_uids: Vec::new(),
                const_ref: None,
                theme_match: None,
                theme_uid: 0,
            });
        }
        self.theme_site = site;
        self.rows_uid = THEME_ROWS;
    }

    /// A `name: value` chunk from a theme row's editor.
    fn theme_apply_chunk(&mut self, cx: &mut Cx, chunk: &str) {
        if let Some((prop, value)) = single_prop_chunk(chunk) {
            if let Err(error) = self.theme_set(cx, &prop, value.trim(), "sidebar") {
                log!("TWEAK theme apply failed: {error}");
            }
        }
    }

    /// Double-click reset: the session-original value back.
    fn theme_reset(&mut self, cx: &mut Cx, name: &str) {
        let original = session()
            .lock()
            .unwrap()
            .diff
            .iter()
            .find(|e| e.scope == "theme" && e.prop == name)
            .map(|e| e.old.clone());
        if let Some(original) = original {
            if let Err(error) = self.theme_set(cx, name, &original, "reset") {
                log!("TWEAK theme reset failed: {error}");
            }
        }
    }

    /// Set one global theme value through [`theme_apply`], the path shared
    /// with the remote op and [`crate::reflect::theme_set_value`]. A colour:
    /// every draw buffer slot holding it is retargeted live, app-wide,
    /// through the pulse's identity ledger (kept in sync after each draw),
    /// and the theme object in the script heap follows, so widgets applied
    /// from now on bake the new colour too. A number: the heap value (see
    /// there). Ledgered at the theme's own definition site with scope
    /// "theme"; undoable, unless origin is an undo/redo replay of a step
    /// already on the stack. The method's own share is the overlay's state:
    /// the palette chips after a colour edit, and the sidebar row for the
    /// value.
    fn theme_set(&mut self, cx: &mut Cx, name: &str, text: &str, origin: &str) -> Result<(), String> {
        let undo = (origin != "undo" && origin != "redo").then_some(track_undo as UndoSink);
        let Some((was, applied)) = theme_apply(cx, name, text, origin, undo)? else {
            return Ok(());
        };
        if matches!(was, ThemeVal::Color(_)) {
            self.theme_colors = theme_palette(cx);
            self.palette_gen = session().lock().unwrap().apply_gen;
        }
        if let Some(row) = self.rows.iter_mut().find(|r| r.prop == name) {
            row.value = applied;
            row.changed = true;
        }
        Ok(())
    }

    fn rebuild_rows(&mut self, cx: &mut Cx, sel_uid: u64, sel_path: &str) {
        let widget = cx.widget_tree().widget(WidgetUid(sel_uid));
        if widget.is_empty() {
            self.rows.clear();
            self.rows_uid = 0;
            self.radius_prop = None;
            return;
        }
        self.rows.clear();
        self.doc_row = None;
        self.sel_is_grid = type_name_of(cx, sel_uid).as_deref() == Some("Grid");
        self.sel_in_grid = cx
            .widget_tree()
            .parent_of(WidgetUid(sel_uid))
            .and_then(|parent| type_name_of(cx, parent.0))
            .as_deref()
            == Some("Grid");
        for (name, value, is_set) in reflect_flat(cx, &widget) {
            let (kind, display, quoted) = if value.starts_with('#') && parse_hex(&value).is_some()
            {
                (RowKind::Color, value, false)
            } else if value.parse::<f64>().is_ok() {
                (RowKind::Num, value, false)
            } else if value == "true" || value == "false" {
                (RowKind::Bool, value, false)
            } else if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                (RowKind::Text, value[1..value.len() - 1].to_string(), true)
            } else {
                (RowKind::Text, value, false)
            };
            let section = classify_prop(&name, &display);
            let (struct_kind, comp_vals) = parse_struct(&display);
            self.rows.push(RowBinding {
                prop: name,
                kind,
                value: display,
                quoted,
                section,
                set: is_set,
                changed: false,
                original: None,
                field_uid: 0,
                swatch_uid: 0,
                struct_kind,
                comp_vals,
                comp_uids: Vec::new(),
                alt_uids: Vec::new(),
                const_ref: None,
                theme_match: None,
                theme_uid: 0,
            });
        }
        // Session-original + resettable flags from the diff log.
        {
            let session = session().lock().unwrap();
            for row in &mut self.rows {
                let mut first_old: Option<&str> = None;
                let mut last_new: Option<&str> = None;
                for entry in session
                    .diff
                    .iter()
                    .filter(|e| e.path == sel_path && e.prop == row.prop)
                {
                    if first_old.is_none() {
                        first_old = Some(&entry.old);
                    }
                    last_new = Some(&entry.new);
                }
                if let (Some(old), Some(new)) = (first_old, last_new) {
                    row.original = Some(old.to_string());
                    row.changed = old != new;
                }
            }
        }
        // The concrete grouped ordering. Stable sort: equal keys keep
        // reflection order, changed rows never move.
        self.rows.sort_by(|a, b| {
            let ka = (a.section.index(), section_rank(a.section, &a.prop));
            let kb = (b.section.index(), section_rank(b.section, &b.prop));
            ka.cmp(&kb)
        });
        // The radius-like style input the corner handles drive: prefer
        // draw_bg's own border_radius, else the first *radius* style row.
        self.radius_prop = self
            .rows
            .iter()
            .find(|row| row.prop == "draw_bg.border_radius" && row.kind == RowKind::Num)
            .or_else(|| {
                self.rows.iter().find(|row| {
                    row.section == SectionKind::Style
                        && row.kind == RowKind::Num
                        && row.prop.split('.').next_back().unwrap_or("").contains("radius")
                        && !row.prop.contains("shadow")
                })
            })
            .and_then(|row| row.value.parse::<f64>().ok().map(|v| (row.prop.clone(), v)));
        // The selection's draw layers, in row order — the material cards.
        self.materials.clear();
        for row in &self.rows {
            if row.section == SectionKind::Style {
                let first = row.prop.split('.').next().unwrap_or("");
                if first.starts_with("draw_") && !self.materials.iter().any(|m| m == first) {
                    self.materials.push(first.to_string());
                }
            }
        }
        // The CASCADE section: read-only rows, one block per construction
        // level, pushed after the sort so they keep exactly this order.
        {
            let widget = cx.widget_tree().widget(WidgetUid(sel_uid));
            self.row_docs = collect_row_docs(cx, &widget);
            // SHADER CONSTANTS: the annotated literals inside each draw
            // layer's fn bodies — actual values IN shader code, listed
            // per layer with the annotation as their doc.
            let materials = self.materials.clone();
            for (mi, layer) in materials.iter().enumerate() {
                let mut seen: Vec<String> = Vec::new();
                for (_, _, name, doc, initial, value, loc) in layer_consts(cx, &widget, layer, mi == 0) {
                    // One knob per name: a literal annotated at two sites
                    // in the same shader is one constant (both patch).
                    if seen.contains(&name) {
                        continue;
                    }
                    seen.push(name.clone());
                    let prop = if self.rows.iter().any(|r| r.prop == name) {
                        format!("{layer} \u{00b7} {name}")
                    } else {
                        name.clone()
                    };
                    // The tooltip names the literal's site: these live in shader code.
                    self.row_docs.insert(prop.clone(), if doc.is_empty() { loc } else { format!("{doc}\n{loc}") });
                    self.rows.push(RowBinding {
                        prop,
                        kind: RowKind::Num,
                        value: fmt_f64(value as f64),
                        quoted: false,
                        section: SectionKind::Style,
                        set: true,
                        changed: value != initial,
                        original: Some(fmt_f64(initial as f64)),
                        field_uid: 0,
                        swatch_uid: 0,
                        struct_kind: StructKind::None,
                        comp_vals: Vec::new(),
                        comp_uids: Vec::new(),
                        alt_uids: Vec::new(),
                        const_ref: Some(ConstRef { layer: layer.clone(), name, initial }),
                        theme_match: None,
                        theme_uid: 0,
                    });
                }
            }
            self.origin_levels.clear();
            let levels = cascade_levels(cx, &widget);
            self.cascade_level_count = levels.len();
            for (i, lvl) in levels.iter().enumerate() {
                for (key, overridden) in &lvl.sets {
                    if !overridden {
                        self.origin_levels.entry(key.clone()).or_insert(i);
                    }
                }
            }
            // The CASCADE section renders these directly (one row per
            // level: icon, chip, file:line, what it sets); the rows list
            // carries no Info rows for it any more.
            self.cascade = levels;
        }
        self.rows_uid = sel_uid;
    }

    /// True when a row is folded into one of LAYOUT's composite rows
    /// (size pair, box editors, spacing/flow, align grid).
    fn layout_composited(prop: &str) -> bool {
        let first = prop.split('.').next().unwrap_or("");
        matches!(
            first,
            "width" | "height" | "min_width" | "max_width" | "min_height" | "max_height" | "aspect"
                | "margin" | "padding" | "spacing" | "wrap_spacing" | "flow" | "align"
                | "distribute" | "abs_pos" | "container_id" | "cell" | "columns" | "rows"
                | "areas" | "column_gap" | "row_gap" | "auto_flow" | "justify_items"
                | "align_items"
        )
    }

    /// The words a LAYOUT composite answers to in the filter box.
    ///
    /// The composite's label is what a person sees and types — "size",
    /// "margin", "align" — and not one of them is a property name. Without
    /// this the filter can only reach the raw `width` / `margin.left` rows
    /// the composite replaced, so typing the name of a row plainly on screen
    /// makes it vanish.
    fn composite_terms(kind: &VisKind) -> &'static str {
        match kind {
            VisKind::Measured => "measured size width height pixels device",
            VisKind::Size => "size width height fit fill min max clamp aspect ratio grow shrink basis weight percent",
            VisKind::BoxInset(BoxKind::Margin) => "margin",
            VisKind::BoxInset(BoxKind::Padding) => "padding",
            VisKind::FlowSpacing => "spacing flow gap wrap direction rows overlay",
            VisKind::AlignGrid => "align alignment justify distribute space between around evenly centre center start end",
            VisKind::Container => "layout container flex grid name cqw cqh",
            VisKind::Absolute => "absolute position abs_pos x y",
            VisKind::GridTracks => "grid columns rows tracks gap areas fill auto_flow fr minmax repeat",
            VisKind::Cell => "cell col row span area place",
            _ => "",
        }
    }

    /// The rows a View lays its children out by, which a Grid ignores.
    fn flow_only(prop: &str) -> bool {
        matches!(
            prop.split('.').next().unwrap_or(""),
            "flow" | "spacing" | "wrap_spacing" | "align" | "distribute"
        )
    }

    /// Which composite, if any, has swallowed `prop`.
    fn composite_of(prop: &str) -> Option<VisKind> {
        match prop.split('.').next().unwrap_or("") {
            "width" | "height" | "min_width" | "max_width" | "min_height" | "max_height" | "aspect" => {
                Some(VisKind::Size)
            }
            "margin" => Some(VisKind::BoxInset(BoxKind::Margin)),
            "padding" => Some(VisKind::BoxInset(BoxKind::Padding)),
            "spacing" | "wrap_spacing" | "flow" => Some(VisKind::FlowSpacing),
            "align" | "distribute" => Some(VisKind::AlignGrid),
            "container_id" => Some(VisKind::Container),
            "abs_pos" => Some(VisKind::Absolute),
            "cell" => Some(VisKind::Cell),
            "columns" | "rows" | "areas" | "column_gap" | "row_gap" | "auto_flow" => {
                Some(VisKind::GridTracks)
            }
            "justify_items" | "align_items" => Some(VisKind::AlignGrid),
            _ => None,
        }
    }

    /// The curated STYLE row set: colors and the handful of numbers a
    /// designer actually reaches for; the long tail folds behind
    /// "show all (N)".
    fn style_curated(row: &RowBinding) -> bool {
        if row.kind == RowKind::Color {
            return true;
        }
        let leaf = row.prop.split('.').next_back().unwrap_or("");
        leaf.contains("radius")
            || leaf.contains("border_size")
            || leaf.contains("font_size")
            || leaf.contains("shadow")
    }


    fn row_index(&self, prop: &str) -> Option<usize> {
        self.rows.iter().position(|row| row.prop == prop)
    }

    fn row_value(&self, prop: &str) -> Option<&str> {
        self.rows
            .iter()
            .find(|row| row.prop == prop)
            .map(|row| row.value.as_str())
    }

    /// The selection's cell as the rows say it.
    fn cell_text(&self) -> CellText {
        let num = |key: &str| {
            self.row_value(&format!("cell.{key}"))
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
                .max(0.0) as u32
        };
        CellText {
            col: num("col"),
            row: num("row"),
            col_span: num("col_span"),
            row_span: num("row_span"),
            area: self
                .row_value("cell.area")
                .map(|v| v.trim_start_matches('@'))
                .filter(|v| *v != "-")
                .unwrap_or("")
                .to_string(),
        }
    }

    /// The cell with one field -- col, row, a span, the area -- replaced by
    /// what was typed, as the chunk that places the whole of it. An area
    /// that is not an id is not sent.
    fn cell_chunk(&self, key: &str, typed: &str) -> Option<String> {
        let mut cell = self.cell_text();
        match key {
            "area" => {
                let name = typed.trim().trim_start_matches('@');
                let valid = name.is_empty()
                    || (name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && !name.starts_with(|c: char| c.is_ascii_digit()));
                if !valid {
                    return None;
                }
                cell.area = name.to_string();
            }
            _ => {
                let value = typed.trim().parse::<f64>().ok()?.max(0.0) as u32;
                match key {
                    "col" => cell.col = value,
                    "row" => cell.row = value,
                    "col_span" => cell.col_span = value,
                    "row_span" => cell.row_span = value,
                    _ => return None,
                }
            }
        }
        Some(cell.chunk())
    }

    /// The absolute position with one coordinate replaced.
    fn abs_chunk(&self, key: &str, value: f64) -> String {
        let (x, y) = self.row_value("abs_pos").and_then(parse_vec2_text).unwrap_or((0.0, 0.0));
        let (x, y) = if key == "x" { (value, y) } else { (x, value) };
        format!("abs_pos: vec2({}, {})", fmt_f64(x), fmt_f64(y))
    }

    /// The axis's Fill with one field -- weight, shrink, basis -- replaced
    /// by what was typed, as the chunk that sets the whole of it.
    fn fill_chunk(&self, axis: &str, key: &str, typed: &str) -> String {
        let mut fill = match parse_size_text(self.row_value(axis).unwrap_or("")) {
            SizeText::Fill(fill) => fill,
            _ => FillText::plain(),
        };
        match key {
            "weight" => fill.weight = typed.trim().parse().unwrap_or(fill.weight),
            "shrink" => fill.shrink = typed.trim().parse().unwrap_or(fill.shrink),
            "basis" => fill.basis = typed.trim().trim_matches('"').trim().to_string(),
            _ => {}
        }
        fill.chunk(axis)
    }

    /// A row matches the filter on its name, its value text, or its
    /// `/** */` annotation (docs are searchable: "banding" finds
    /// color_dither through its doc line).
    fn row_matches_filter(&self, row: &RowBinding) -> bool {
        self.filter.is_empty()
            || row.prop.to_lowercase().contains(&self.filter)
            || row.value.to_lowercase().contains(&self.filter)
            || self
                .row_docs
                .get(&row.prop)
                .is_some_and(|doc| doc.to_lowercase().contains(&self.filter))
    }

    /// The visible entry list: sections in order, folded sections
    /// collapsed, LAYOUT compacted into the composite row grammar (size
    /// pair, box editors, spacing/flow, align grid), the long tails behind
    /// "show all (N)". The filter searches across all sections at once on
    /// names, values and annotation text; while filtering, curation
    /// suspends (a long-tail match shows regardless) and sections stay
    /// open.
    fn build_visible(&self) -> Vec<VisKind> {
        let filtering = !self.filter.is_empty();
        let mut out = Vec::new();
        // WHAT IT IS, before what it looks like: the selection's own name —
        // editable, because "this one needs a name" is the commonest thing
        // to want to say about an anonymous widget — and its type.
        if !filtering && self.panel_tab != PanelTab::Theme && self.rows_uid != 0 {
            out.push(VisKind::Identity);
        }
        // SHADER CONSTANTS: the annotated literals inside the draw layers'
        // fn bodies — "actual values IN shader code" — each with its doc
        // line. Absent when the widget's shaders carry none. (Annotated
        // props are not constants: they stay in their sections with the
        // gold marker and the doc line under the touched row.)
        if !filtering {
            let tweak: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.const_ref.is_some())
                .map(|(i, _)| i)
                .collect();
            if !tweak.is_empty() {
                out.push(VisKind::TweakHeader(tweak.len(), self.tweakables_open));
                if self.tweakables_open {
                    for index in tweak {
                        out.push(VisKind::Tweakable(index));
                    }
                }
            }
        }
        for section in SECTION_ORDER {
            if section == SectionKind::Cascade {
                if filtering || self.cascade.is_empty() {
                    continue;
                }
                let open = !self.collapsed[section.index()];
                out.push(VisKind::Section(section, self.cascade.len(), open));
                if open {
                    for i in 0..self.cascade.len() {
                        out.push(VisKind::CascadeLevel(i));
                    }
                }
                continue;
            }
            let members: Vec<usize> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| row.section == section && row.const_ref.is_none() && self.row_matches_filter(row))
                .map(|(index, _)| index)
                .collect();
            let theme = self.panel_tab == PanelTab::Theme;
            let composites: Vec<VisKind> = if section == SectionKind::Layout && !theme {
                // A filter NARROWS the panel; it does not dismantle its
                // grammar. A composite survives filtering when the words it
                // answers to match — so "size" finds the size row rather
                // than emptying the section.
                let wanted = |kind: &VisKind| {
                    !filtering || Self::composite_terms(kind).contains(self.filter.as_str())
                };
                let mut list = Vec::new();
                // The measurement first, then the controls that produced
                // it: read what it IS, then change what it asks for.
                list.push(VisKind::Measured);
                // The first half is the selection in its parent: its size,
                // its margin, whether it sits in the flow at all.
                list.push(VisKind::Group(GroupKind::Item));
                // Always present: an axis with no reflected row IS the Fit
                // state — the segments must still show it.
                list.push(VisKind::Size);
                if self.rows.iter().any(|r| r.prop.starts_with("margin")) {
                    list.push(VisKind::BoxInset(BoxKind::Margin));
                }
                list.push(VisKind::Absolute);
                if self.sel_in_grid {
                    list.push(VisKind::Cell);
                }
                // The second half is what it holds. A spacing row is what
                // makes it a container; a Label flows its text and has a
                // flow row, but nothing to space. A Grid places its
                // children by track, so it shows tracks where a View shows
                // a flow, and its cells' alignment where a View shows the
                // children's.
                if self.row_index("spacing").is_some() {
                    list.push(VisKind::Group(GroupKind::Container));
                    list.push(VisKind::Container);
                }
                if self.sel_is_grid {
                    list.push(VisKind::GridTracks);
                    list.push(VisKind::AlignGrid);
                } else {
                    if self.row_index("spacing").is_some() || self.row_index("flow").is_some() {
                        list.push(VisKind::FlowSpacing);
                    }
                    if self.row_index("align.x").is_some() {
                        list.push(VisKind::AlignGrid);
                    }
                }
                if self.rows.iter().any(|r| r.prop.starts_with("padding")) {
                    list.push(VisKind::BoxInset(BoxKind::Padding));
                }
                list.retain(|kind| wanted(kind));
                list
            } else {
                Vec::new()
            };
            if members.is_empty() && composites.is_empty() {
                continue;
            }
            let open = filtering || !self.collapsed[section.index()];
            out.push(VisKind::Section(section, members.len(), open));
            if !open {
                continue;
            }
            out.extend(composites.iter().copied());
            let expanded = filtering || self.expanded[section.index()];
            // A section with no primary row read as an empty header over a
            // "show all": lead with its first three rows instead.
            let primary = members
                .iter()
                .filter(|&&i| match section {
                    SectionKind::Style => theme || Self::style_curated(&self.rows[i]),
                    SectionKind::Layout => theme || !Self::layout_composited(&self.rows[i].prop),
                    _ => true,
                })
                .count();
            let force_show = if !filtering && !expanded && primary == 0 { 3usize } else { 0 };
            let mut forced = 0usize;
            let mut hidden = 0usize;
            let mut last_material: Option<String> = None;
            for index in members {
                let row = &self.rows[index];
                // Theme rows are all primary: nothing is folded behind a
                // "show all".
                let in_tail = match section {
                    _ if theme => false,
                    SectionKind::Layout => {
                        // Folded away exactly when the composite that owns
                        // it is on screen — filtering included, or a filter
                        // that keeps the size row would print width and
                        // height a second time underneath it.
                        Self::composite_of(&row.prop)
                            .is_some_and(|kind| composites.iter().any(|c| c.same_row(&kind)))
                            || (!expanded && !Self::layout_composited(&row.prop))
                            || (self.sel_is_grid && Self::flow_only(&row.prop))
                    }
                    SectionKind::Style => !expanded && !Self::style_curated(row),
                    _ => false,
                };
                if filtering && in_tail {
                    continue;
                }
                if !filtering && in_tail && forced < force_show {
                    forced += 1;
                } else if !filtering && in_tail {
                    // Rows folded into composites never re-appear; the
                    // rest count toward the expander.
                    if !(section == SectionKind::Layout && Self::layout_composited(&row.prop))
                        && !(section == SectionKind::Style && Self::style_curated(row))
                    {
                        hidden += 1;
                    }
                    continue;
                }
                // Material card header before each draw layer's first row.
                if section == SectionKind::Style && !filtering {
                    let first = row.prop.split('.').next().unwrap_or("");
                    if first.starts_with("draw_") && last_material.as_deref() != Some(first) {
                        if let Some(mi) = self.materials.iter().position(|m| m == first) {
                            out.push(VisKind::Material(mi));
                        }
                        last_material = Some(first.to_string());
                    }
                }
                out.push(VisKind::Prop(index));
            }
            if hidden > 0 {
                out.push(VisKind::More(section, hidden));
            }
        }
        out
    }

    /// Draw the opaque panel backing, the splitter, and the property
    /// sidebar into the vacated band.
    fn draw_sidebar(&mut self, cx: &mut Cx2d, scope: &mut Scope, sel: Option<&TweakPick>) {
        let pass_size = cx.current_pass_size();
        let width = sidebar_width();
        // The band owns its full column, window-top to bottom: the app (tab
        // bars, chrome, anything) must never read through the panel.
        let band = Rect {
            pos: dvec2(pass_size.x - width, 0.0),
            size: dvec2(width, pass_size.y),
        };
        self.band = band;
        // The window's hit-test needs this from outside the widget — see
        // `panel_owns_pointer`. The note card joins it in the overlay draw.
        {
            let mut sess = session().lock().unwrap();
            sess.chrome_band = Some(band);
            sess.isolate_uid = if self.tree_isolate { self.isolate_uid } else { 0 };
        }
        // The panel is flat chrome over the exploded view: pointer events
        // inside the band flow through in plain window coordinates.
        cx.sploded_set_flat_band(Some(band));

        // One solid surface first: the band must never show the app's bare
        // clear color between rows.
        self.draw_panel_bg.draw_abs(cx, band);
        self.draw_splitter.draw_abs(
            cx,
            Rect {
                pos: band.pos,
                size: dvec2(SPLITTER_WIDTH, band.size.y),
            },
        );

        self.ensure_sidebar(cx);
        let sidebar = self.sidebar.as_ref().unwrap().clone();

        if self.panel_tab == PanelTab::Theme {
            let colours = self.rows.iter().filter(|r| r.kind == RowKind::Color).count();
            let footer = sidebar.child(live_id!(ident_footer));
            footer.child(live_id!(title_label)).set_visible(cx, true);
            footer
                .child(live_id!(title_label))
                .set_text(cx, &format!("Theme  \u{2022}  {colours} colours  \u{2022}  {} values", self.rows.len() - colours));
            let site = self.theme_site.split(':').next().unwrap_or("").to_string();
            footer.child(live_id!(path_row)).child(live_id!(path_label)).set_text(cx, &format!("edits land in {site}"));
            footer.child(live_id!(scope_row)).set_visible(cx, false);
        } else {
            sidebar.child(live_id!(ident_footer)).child(live_id!(scope_row)).set_visible(cx, sel.is_some());
        }
        match sel {
            _ if self.panel_tab == PanelTab::Theme => {}
            Some(sel) => {
                // The title line used to say "Label - 29 props": a row of
                // the footer spent on a count nobody acts on. It now shows
                // only while the exploded view is up, and then only the depth
                // readout -- which plane the selection sits on ("is it
                // stacked deep, or is the z-step just insane?").
                let depth = if cx.sploded_active() {
                    cx.sploded_depth_of(sel.uid)
                        .map(|level| format!("L{level}/{}", cx.sploded_max_level()))
                } else {
                    None
                };
                let title = sidebar.child(live_id!(ident_footer)).child(live_id!(title_label));
                title.set_visible(cx, depth.is_some());
                if let Some(depth) = depth {
                    title.set_text(cx, &depth);
                }
                let now = cx.seconds_since_app_start();
                let shown_path = if now < self.footer_copied_until {
                    // The click's receipt, in place of the path it copied.
                    self.next_frame = cx.new_next_frame();
                    "path copied to the clipboard".to_string()
                } else {
                    // The footer shows the REFERENCE itself, not a prettier
                    // rendering of it: this line is what the click copies,
                    // and two spellings of one thing is how a person ends up
                    // pasting something the tools do not accept. Only the
                    // head is clipped, so the tail — the part that says which
                    // widget — always survives.
                    let reference = self.sel_ref(cx, sel.uid);
                    tail_ellipsis(&reference, 48)
                };
                sidebar
                    .child(live_id!(ident_footer))
                    .child(live_id!(path_row))
                    .child(live_id!(path_label))
                    .set_text(cx, &shown_path);
                {
                    let all = session().lock().unwrap().scope_all;
                    let row = sidebar.child(live_id!(ident_footer)).child(live_id!(scope_row));
                    row.child(live_id!(scope_line)).child(live_id!(scope_this)).set_text(cx, "this");
                    row.child(live_id!(scope_line)).child(live_id!(scope_all)).set_text(cx, &format!("all {}s", sel.ty));
                    set_button_fill(cx, row.child(live_id!(scope_line)).child(live_id!(scope_this)), !all);
                    set_button_fill(cx, row.child(live_id!(scope_line)).child(live_id!(scope_all)), all);
                    // The modifier: confine an `all` fan-out to the isolated
                    // branch. Lit when it is ON and there is an isolation for
                    // it to be on ABOUT; the label greys when there is not,
                    // so a dark button is never ambiguous between "off" and
                    // "nothing here to act on".
                    let isolating = self.tree_isolate && self.isolate_uid != 0;
                    let on = !session().lock().unwrap().scope_unconfined;
                    let confined = isolating && on;
                    {
                        let btn = row.child(live_id!(scope_line)).child(live_id!(scope_isolated));
                        set_button_fill(cx, btn.clone(), confined);
                        let mut btn = btn;
                        let color: Vec4f = if isolating && all {
                            vec4(0.92, 0.92, 0.94, 1.0)
                        } else {
                            vec4(0.42, 0.42, 0.45, 1.0)
                        };
                        script_apply_eval!(cx, btn, { draw_text +: { color: #(color) } });
                    }
                    let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
                    // Same three-way answer the apply itself gives: a
                    // confined "all" does not touch the type's definition,
                    // so the footer must not promise that it does.
                    let root = cx.widget_tree().widget(WidgetUid(self.isolate_uid));
                    let origin = if widget.is_empty() {
                        String::new()
                    } else if all && confined && !root.is_empty() {
                        source_origin(cx, &root)
                    } else if all {
                        type_origin(cx, &widget)
                    } else {
                        source_origin(cx, &widget)
                    };
                    let base = origin.rsplit('/').next().unwrap_or(&origin).to_string();
                    // The path line below is the widget's ADDRESS in the
                    // running tree; this line is the FILE an edit is written
                    // to, which changes with the scope -- "this" edits where
                    // the instance is declared, "all Labels" edits the Label
                    // type where it is defined. Two different questions, and
                    // a line that only named the file read as a contradiction
                    // of the path under it.
                    let line = if base.is_empty() {
                        String::new()
                    } else if all && confined && !root.is_empty() {
                        format!("edits land in {base} (the isolated branch)")
                    } else if all {
                        format!("edits land in {base} (the {} type, so every {})", sel.ty, sel.ty)
                    } else {
                        format!("edits land in {base} (this instance)")
                    };
                    row.child(live_id!(scope_origin)).set_text(cx, &line);
                }
            }
            None => {
                let title = sidebar.child(live_id!(ident_footer)).child(live_id!(title_label));
                title.set_visible(cx, true);
                title.set_text(cx, "tweak");
                sidebar
                    .child(live_id!(ident_footer)).child(live_id!(path_row)).child(live_id!(path_label))
                    .set_text(cx, "click a widget to inspect it");
            }
        }
        self.search_uid = sidebar
            .child(live_id!(filter_row))
            .child(live_id!(search))
            .child(live_id!(input))
            .widget_uid()
            .0;
        {
            // The two that were never lit: the exploded-view toggle and the
            // note. Both are modes you can be IN, and a mode you cannot see
            // yourself in is the same complaint as the scope row's.
            let sploded = sidebar.child(live_id!(filter_row)).child(live_id!(sploded));
            self.sploded_uid = sploded.widget_uid().0;
            let armed = cx.sploded_will_be_active();
            set_button_fill(cx, sploded, armed);
        }
        {
            let select = sidebar.child(live_id!(filter_row)).child(live_id!(select));
            self.select_uid = select.widget_uid().0;
            let picking = !session().lock().unwrap().selection_locked;
            set_button_fill(cx, select, picking);
        }
        self.scope_this_uid = sidebar.child(live_id!(ident_footer)).child(live_id!(scope_row)).child(live_id!(scope_line)).child(live_id!(scope_this)).widget_uid().0;
        self.scope_all_uid = sidebar.child(live_id!(ident_footer)).child(live_id!(scope_row)).child(live_id!(scope_line)).child(live_id!(scope_all)).widget_uid().0;
        self.scope_isolated_uid = sidebar.child(live_id!(ident_footer)).child(live_id!(scope_row)).child(live_id!(scope_line)).child(live_id!(scope_isolated)).widget_uid().0;
        if self.focus_search_pending {
            let input = sidebar
                .child(live_id!(filter_row))
                .child(live_id!(search))
                .child(live_id!(input));
            if input.area() != Area::Empty {
                cx.set_key_focus(input.area());
                self.focus_search_pending = false;
            }
        }
        // The panel tabs: Props / Shader / Tree / Theme / Spec, one content
        // visible.
        {
            let tab = self.panel_tab;
            sidebar
                .child(live_id!(props_wrap))
                .set_visible(cx, matches!(tab, PanelTab::Props | PanelTab::Theme));
            sidebar
                .child(live_id!(shader_col))
                .set_visible(cx, tab == PanelTab::Shader);
            sidebar
                .child(live_id!(tree_wrap))
                .set_visible(cx, tab == PanelTab::Tree);
            sidebar
                .child(live_id!(spec_col))
                .set_visible(cx, tab == PanelTab::Spec);
            if tab == PanelTab::Tree {
                // The toggle shows its state by fill, like the scope buttons,
                // and says what it is isolating — a tree cut down to one
                // subtree must announce that it is cut down.
                let head = sidebar.child(live_id!(tree_wrap)).child(live_id!(tree_head));
                set_button_fill(cx, head.child(live_id!(isolate)), self.tree_isolate);
                set_button_fill(cx, head.child(live_id!(center)), self.view_center);
                let zoom_field = head.child(live_id!(zoom));
                if zoom_field.area() == Area::Empty || !cx.has_key_focus(zoom_field.area()) {
                    // A FabValueInput holds a number, not a string: set_text
                    // leaves its own value at zero and the field reads 0.00.
                    zoom_field
                        .as_fab_value_input()
                        .set_value(cx, self.view_zoom.max(1.0) as f64);
                }
                let hint = match (self.tree_isolate, sel.as_ref()) {
                    (true, Some(sel)) => format!("{} and what is inside it", sel.ty),
                    (true, None) => "select something to isolate".to_string(),
                    (false, _) => String::new(),
                };
                head.child(live_id!(isolate_hint)).set_text(cx, &hint);
            }
            {
                // The filter works on the row list, which the Shader and
                // Spec tabs do not have. A box that looks live and does
                // nothing is worse than none, so on those tabs it says so
                // and goes quiet.
                let filters_here = !matches!(tab, PanelTab::Shader | PanelTab::Spec);
                let input = sidebar
                    .child(live_id!(filter_row))
                    .child(live_id!(search))
                    .child(live_id!(input));
                if let Some(mut input) = input.borrow_mut::<crate::TextInput>() {
                    let hint = if filters_here { "Filter" } else { "no filter on this tab" };
                    if input.empty_text() != hint {
                        input.set_empty_text(cx, hint.to_string());
                    }
                }
                let text: Vec4f = if filters_here {
                    vec4(0.90, 0.90, 0.92, 1.0)
                } else {
                    vec4(0.42, 0.42, 0.46, 1.0)
                };
                let bg: Vec4f = if filters_here {
                    vec4(0.106, 0.106, 0.106, 1.0)
                } else {
                    vec4(0.16, 0.16, 0.17, 1.0)
                };
                let mut input = input;
                script_apply_eval!(cx, input, {
                    draw_text +: { color: #(text) }
                    draw_bg +: { color: #(bg) }
                });
            }
            let tab_row = sidebar.child(live_id!(tab_row));
            let tabs = [
                (live_id!(tab_props), PanelTab::Props, "Props"),
                (live_id!(tab_shader), PanelTab::Shader, "Shader"),
                (live_id!(tab_tree), PanelTab::Tree, "Tree"),
                (live_id!(tab_theme), PanelTab::Theme, "Theme"),
                (live_id!(tab_spec), PanelTab::Spec, "Spec"),
            ];
            for (i, (id, t, label)) in tabs.into_iter().enumerate() {
                let btn = tab_row.child(id);
                btn.set_text(cx, label);
                set_button_fill(cx, btn.clone(), t == tab);
                self.tab_uids[i] = btn.widget_uid().0;
            }
            if tab == PanelTab::Spec {
                self.draw_spec(cx, sel);
            } else if self.spec_dirty {
                // Left the tab with something unsaved: flush it now rather
                // than wait for a focus change that may never come.
                self.spec_flush();
            }
            // The prompt strip lives OUTSIDE the tabs: it is pinned between
            // the tab bodies and the footer, so whichever tab is up, the box
            // is in the same place and the body scrolls above it.
            {
                let row = sidebar.child(live_id!(prompt_row));
                let bar = row.child(live_id!(prompt_bar));
                self.prompt_queue_uid = bar.child(live_id!(queue)).widget_uid().0;
                self.prompt_send_uid = bar.child(live_id!(send)).widget_uid().0;
                // On the Shader tab the box is a shader ask and the status
                // line is that channel's -- "waiting", "live", or the error.
                let shader_tab = tab == PanelTab::Shader;
                let status = {
                    let s = session().lock().unwrap();
                    if shader_tab && !s.vibe_status.is_empty() {
                        s.vibe_status.clone()
                    } else {
                        s.prompt_status.clone()
                    }
                };
                bar.child(live_id!(prompt_status)).set_text(cx, &status);
                let field = row.child(live_id!(prompt_field));
                self.prompt_field_uid = field.widget_uid().0;
                if let Some(mut input) = field.borrow_mut::<crate::TextInput>() {
                    let hint = if shader_tab {
                        let layer = self.vibe_layer.clone().unwrap_or_else(|| "draw_bg".to_string());
                        format!("what should {layer}'s code do differently\u{2026} Ctrl+Enter sends \u{00b7} colours and sizes stay in Props")
                    } else {
                        "what should change\u{2026} Ctrl+Enter sends \u{00b7} Alt+Enter queues".to_string()
                    };
                    if input.empty_text() != hint {
                        input.set_empty_text(cx, hint);
                    }
                }
                if self.prompt_clear_field {
                    // A send emptied the box. Clear it here rather than in
                    // the send, so the field is written exactly once a frame
                    // and never while the caret is mid-keystroke.
                    self.prompt_clear_field = false;
                    field.set_text(cx, "");
                } else if field.area() == Area::Empty || !cx.has_key_focus(field.area()) {
                    // The anti-clobber guard: without it, typing is
                    // overwritten from the session every frame.
                    let live = session().lock().unwrap().prompt.clone();
                    if field.text() != live {
                        field.set_text(cx, &live);
                    }
                }
            }
            // Shader tab content: the layer's live preview + doc + prompt.
            if tab == PanelTab::Shader {
                // Default to the selection's first draw layer (not every
                // widget has a draw_bg).
                let layer = self
                    .vibe_layer
                    .clone()
                    .or_else(|| self.materials.first().cloned())
                    .unwrap_or_else(|| "draw_bg".to_string());
                let col = sidebar.child(live_id!(shader_col));
                // The layer the Shader tab shows is the one a prompt or an
                // editor apply targets.
                self.vibe_layer = Some(layer.clone());
                col.child(live_id!(shader_title)).set_text(cx, &layer);
                let doc = self.row_docs.get(&layer).cloned().unwrap_or_default();
                // One line: the label ellipsises at its own width (max_lines
                // 1); the whole doc rides on hover as a tooltip.
                let mut doc_line = doc.lines().next().unwrap_or("").trim().to_string();
                // A layer with no live draw call (a View with show_bg off,
                // reached by the click-to-climb) has nothing to mirror: say
                // so where the well would otherwise sit empty and silent.
                {
                    let widget = cx.widget_tree().widget(WidgetUid(self.rows_uid));
                    let primary = self.materials.first().is_some_and(|m| *m == layer);
                    if !widget.is_empty() && layer_shader_id(cx, &widget, &layer, primary).is_none() {
                        doc_line = format!("{layer} is not drawn: no live draw call to mirror (show_bg off?)");
                    }
                }
                col.child(live_id!(shader_doc)).set_text(cx, &doc_line);
                // Flowing text: the source comment's hard breaks are not
                // paragraph breaks.
                let doc_full = doc.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
                self.shader_doc_full = if doc_full.chars().count() > 40 || doc.trim().lines().count() > 1 {
                    doc_full
                } else {
                    String::new()
                };
                self.shader_src_uid = col
                    .child(live_id!(src_scroll))
                    .child(live_id!(shader_src))
                    .widget_uid()
                    .0;
                // The source editor unfolds on demand only; folded, the tab
                // is the swatch + doc + prompt.
                let fold = col.child(live_id!(src_fold));
                self.shader_fold_uid = fold.widget_uid().0;
                fold.set_text(
                    cx,
                    if self.shader_src_open { "- source" } else { "+ source" },
                );
                col.child(live_id!(src_scroll)).set_visible(cx, self.shader_src_open);
                // The shader as written: pixel (and vertex when the layer
                // sets its own) with their docs — what the prompt rewrites.
                {
                    let widget = cx.widget_tree().widget(WidgetUid(self.rows_uid));
                    let fns = if widget.is_empty() {
                        Vec::new()
                    } else {
                        layer_fn_sources(cx, &widget, &layer)
                    };
                    let mut text = String::new();
                    let overrides = session().lock().unwrap().fn_overrides.clone();
                    for (name, loc, src) in &fns {
                        if !text.is_empty() {
                            text.push_str("\n\n");
                        }
                        // A fn applied live from this view shows as applied,
                        // not as the file has it.
                        let key = (self.rows_uid, layer.clone(), name.clone());
                        let body = overrides.get(&key).cloned().unwrap_or_else(|| reindent_two(src));
                        text.push_str(&format!("// {name} \u{2014} {loc}\n{body}"));
                    }
                    if text.is_empty() {
                        text = "(no script-defined pixel/vertex fn on this layer \u{2014} the type's own shader)".to_string();
                    }
                    self.vibe_fn_sources = fns;
                    let editor = col.child(live_id!(src_scroll)).child(live_id!(shader_src));
                    // While a person types here, the view is the source of
                    // truth: same widget+layer, no outside apply since, and
                    // the editor focused or holding exactly what it last
                    // applied (a failed edit stays on screen to be fixed).
                    let key = (self.rows_uid, layer.clone());
                    let external = session().lock().unwrap().fn_override_gen;
                    let typing = self.live_key == key
                        && self.fn_external_seen == external
                        && (cx.has_key_focus(editor.area()) || editor.text() == self.live_last_applied);
                    if editor.text() != text && !typing {
                        editor.set_text(cx, &text);
                        self.live_last_applied = text.clone();
                        self.live_last_good = text.clone();
                    }
                    self.live_key = key;
                    self.fn_external_seen = external;
                }
                let sw_ref = col.child(live_id!(big));
                let sw_opt = sw_ref.borrow_mut::<TweakMaterialSwatch>();
                if let Some(mut sw) = sw_opt {
                    sw.clip = self.swatch_clip;
                    if sw.applied_gen != self.rows_gen || sw.layer != layer || sw.mirror_uid != self.rows_uid {
                        let widget = cx.widget_tree().widget(WidgetUid(self.rows_uid));
                        if !widget.is_empty() {
                            sw.mirror_uid = widget.widget_uid().0;
                            sw.mirror_primary = self.materials.first().is_some_and(|m| *m == layer);
                            sw.mirror_layer = layer.clone();
                            sw.applied_gen = self.rows_gen;
                            sw.layer = layer;
                        }
                    }
                }
                // The animator's states, each a small posed well.
                self.refresh_state_swatches(cx, &col);
                // Under the wells: this layer's SHADER CONSTANTS, then its
                // INPUTS (uniforms/instances; annotated first), then source.
                {
                    let layer = self.vibe_layer.clone().unwrap_or_else(|| "draw_bg".to_string());
                    let mut ents = Vec::new();
                    let consts: Vec<usize> = self
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.const_ref.as_ref().is_some_and(|c| c.layer == layer))
                        .map(|(i, _)| i)
                        .collect();
                    if !consts.is_empty() {
                        ents.push(VisKind::TweakHeader(consts.len(), true));
                        for i in consts {
                            ents.push(VisKind::Tweakable(i));
                        }
                    }
                    let prefix = format!("{layer}.");
                    let mut inputs: Vec<usize> = self
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| {
                            r.const_ref.is_none()
                                && r.kind != RowKind::Info
                                && r.section != SectionKind::Cascade
                                && r.prop.starts_with(&prefix)
                        })
                        .map(|(i, _)| i)
                        .collect();
                    inputs.sort_by_key(|i| if self.row_docs.contains_key(&self.rows[*i].prop) { 0 } else { 1 });
                    if !inputs.is_empty() {
                        ents.push(VisKind::InputsHeader(inputs.len()));
                        for i in inputs {
                            ents.push(VisKind::Prop(i));
                        }
                    }
                    col.child(live_id!(shader_rows_wrap)).set_visible(cx, !ents.is_empty());
                    self.shader_entries = ents;
                }
            }
            // Tree tab data: refresh on generation change.
            if tab == PanelTab::Tree && self.tree_rows_gen != self.rows_gen.wrapping_add(1)
            {
                self.tree_rows = cx.widget_tree().flat_tree(cx);
                self.tree_rows_gen = self.rows_gen.wrapping_add(1);
                // Parent links from the depth-first order: the nearest
                // earlier row one level up.
                let mut stack: Vec<usize> = Vec::new();
                self.tree_parents = self
                    .tree_rows
                    .iter()
                    .enumerate()
                    .map(|(i, row)| {
                        while let Some(&top) = stack.last() {
                            if self.tree_rows[top].depth >= row.depth {
                                stack.pop();
                            } else {
                                break;
                            }
                        }
                        let parent = stack.last().copied();
                        stack.push(i);
                        parent
                    })
                    .collect();
                self.tree_children = vec![Vec::new(); self.tree_rows.len()];
                for (i, parent) in self.tree_parents.iter().enumerate() {
                    if let Some(parent) = parent {
                        self.tree_children[*parent].push(i);
                    }
                }
                self.tree_open_defaults_pending = true;
            }
        }

        self.composite_fields.clear();
        self.composite_clicks.clear();
        self.open_popup = None;
        session().lock().unwrap().popup = None;
        let entries_all = self.build_visible();
        let shader_entries = self.shader_entries.clone();
        let mut visible_rects: Vec<VisRow> = Vec::with_capacity(entries_all.len());

        let walk = Walk::abs_rect(Rect {
            pos: dvec2(band.pos.x + SPLITTER_WIDTH, band.pos.y),
            size: dvec2(band.size.x - SPLITTER_WIDTH, band.size.y),
        });
        self.tree_visible.clear();
        while let Some(step_widget) = sidebar.draw_walk(cx, scope, walk).step() {
            // The tree tab's list fills from the flattened hierarchy.
            if step_widget.widget_uid().0 == self.tree_list_uid {
                let sel_uid = self.rows_uid;
                // Filter: matching nodes plus their ancestor chain stay
                // visible; their folders force open while filtering.
                let filtering = !self.filter.is_empty();
                let keep: Option<Vec<bool>> = if filtering {
                    let mut keep = vec![false; self.tree_rows.len()];
                    for (i, row) in self.tree_rows.iter().enumerate() {
                        if row.name.to_lowercase().contains(&self.filter)
                            || row.ty.to_lowercase().contains(&self.filter)
                        {
                            let mut cursor = Some(i);
                            while let Some(index) = cursor {
                                if keep[index] {
                                    break;
                                }
                                keep[index] = true;
                                cursor = self.tree_parents.get(index).copied().flatten();
                            }
                        }
                    }
                    Some(keep)
                } else {
                    None
                };
                let Some(mut tree) = step_widget.borrow_mut::<FileTree>() else {
                    continue;
                };
                // First fill (or selection change): open the levels that
                // make the tree readable / reveal the selection.
                if self.tree_open_defaults_pending {
                    self.tree_open_defaults_pending = false;
                    for row in &self.tree_rows {
                        if row.has_children && row.depth < 4 {
                            tree.set_folder_is_open(cx, LiveId(row.uid), true, Animate::No);
                        }
                    }
                }
                if self.tree_scrolled_uid != sel_uid && sel_uid != 0 {
                    // The pin (by any route: body click, 3D pick, remote
                    // uid apply, undo) IS the tree selection.
                    tree.select_node(cx, LiveId(sel_uid));
                    if let Some(index) =
                        self.tree_rows.iter().position(|row| row.uid == sel_uid)
                    {
                        let mut cursor = self.tree_parents.get(index).copied().flatten();
                        while let Some(parent) = cursor {
                            tree.set_folder_is_open(
                                cx,
                                LiveId(self.tree_rows[parent].uid),
                                true,
                                Animate::No,
                            );
                            cursor = self.tree_parents.get(parent).copied().flatten();
                        }
                        // Scroll into view: the servo below measures and
                        // corrects on the next draws.
                        self.tree_scroll_tries = Some(8);
                    }
                    self.tree_scrolled_uid = sel_uid;
                }
                if self.tree_scroll_tries.is_some() && sel_uid != 0 {
                    tree.begin_reveal(LiveId(sel_uid));
                }
                if filtering {
                    if let Some(keep) = &keep {
                        for (i, row) in self.tree_rows.iter().enumerate() {
                            if keep[i] && row.has_children {
                                tree.set_folder_is_open(
                                    cx,
                                    LiveId(row.uid),
                                    true,
                                    Animate::No,
                                );
                            }
                        }
                    }
                }
                // Recursive emission over the flattened rows.
                fn emit(
                    tree: &mut FileTree,
                    cx: &mut Cx2d,
                    rows: &[crate::widget_tree::FlatTreeRow],
                    children: &[Vec<usize>],
                    keep: Option<&Vec<bool>>,
                    locked: u64,
                    pinned: &[u64],
                    index: usize,
                ) {
                    if let Some(keep) = keep {
                        if !keep[index] {
                            return;
                        }
                    }
                    let row = &rows[index];
                    // The row isolation is LOCKED to wears a mark, so it stays
                    // obvious which one the view is held on even after the
                    // selection has moved to a child inside it.
                    // A widget carrying a PINNED note wears the same mark
                    // here as it does on the canvas: the tree is the other
                    // way of finding what has already been said about what.
                    let mark = if pinned.contains(&row.uid) {
                        "\u{1f4cc} "
                    } else {
                        ""
                    };
                    let label = if row.uid == locked {
                        format!("\u{25c9} {mark}{} \u{00b7} {}", row.name, row.ty)
                    } else {
                        format!("{mark}{} \u{00b7} {}", row.name, row.ty)
                    };
                    if row.has_children {
                        if tree.begin_folder(cx, LiveId(row.uid), &label).is_ok() {
                            for &child in &children[index] {
                                emit(tree, cx, rows, children, keep, locked, pinned, child);
                            }
                            tree.end_folder();
                        }
                    } else {
                        tree.file(cx, LiveId(row.uid), &label);
                    }
                }
                // The pin marks come from the same place the canvas badges
                // do, refreshed on the same throttle — but the tree shows
                // them whether or not a note card happens to be open.
                self.refresh_badges(cx);
                let pinned_uids: Vec<u64> =
                    self.badge_targets.iter().map(|(uid, _, _)| *uid).collect();
                // Isolate: one root — the selection — instead of the app's.
                // With nothing selected there is nothing to isolate, so the
                // whole tree stands.
                let isolate_root = if self.tree_isolate && self.isolate_uid != 0 {
                    self.tree_rows
                        .iter()
                        .position(|row| row.uid == self.isolate_uid)
                } else {
                    None
                };
                match isolate_root {
                    Some(root) => {
                        // The isolated root has to be open or the subtree it
                        // was opened for is exactly what stays hidden.
                        if self.tree_rows[root].has_children {
                            tree.set_folder_is_open(
                                cx,
                                LiveId(self.tree_rows[root].uid),
                                true,
                                Animate::No,
                            );
                        }
                        emit(
                            &mut tree,
                            cx,
                            &self.tree_rows,
                            &self.tree_children,
                            keep.as_ref(),
                            self.isolate_uid,
                            &pinned_uids,
                            root,
                        );
                    }
                    None => {
                        for index in 0..self.tree_rows.len() {
                            if self.tree_parents[index].is_none() {
                                emit(
                                    &mut tree,
                                    cx,
                                    &self.tree_rows,
                                    &self.tree_children,
                                    keep.as_ref(),
                                    self.isolate_uid,
                                    &pinned_uids,
                                    index,
                                );
                            }
                        }
                    }
                }
                if let Some(tries) = self.tree_scroll_tries {
                    let vp = self.props_viewport.unwrap_or(band);
                    match tree.take_reveal_y() {
                        Some(y) if tries > 0 => {
                            let top = vp.pos.y + 8.0;
                            let bottom = vp.pos.y + vp.size.y - 40.0;
                            if y < top || y > bottom {
                                tree.scroll_by(cx, y - (vp.pos.y + vp.size.y * 0.33));
                                tree.redraw(cx);
                                self.tree_scroll_tries = Some(tries - 1);
                            } else {
                                self.tree_scroll_tries = None;
                            }
                        }
                        _ => {
                            self.tree_scroll_tries = None;
                        }
                    }
                }
                continue;
            }
            let in_shader_tab = step_widget.widget_uid().0 == self.shader_list_uid;
            let list_viewport = self.props_viewport;
            let Some(mut list) = step_widget.borrow_mut::<PortalList>() else {
                continue;
            };
            let entries = if in_shader_tab { &shader_entries } else { &entries_all };
            list.set_item_range(cx, 0, entries.len());
            // A row can draw twice (TWEAKABLES + its home section); every
            // draw registers its fields, so the sets start empty per frame.
            for row in &mut self.rows {
                row.comp_uids.clear();
                row.alt_uids.clear();
            }
            while let Some(entry_id) = list.next_visible_item(cx) {
                if entry_id >= entries.len() {
                    continue;
                }
                let entry = entries[entry_id];
                let template = match entry {
                    VisKind::Section(..) => live_id!(SectionRow),
                    VisKind::More(..) => live_id!(MoreRow),
                    VisKind::Material(_) => live_id!(MaterialRow),
                    VisKind::Size => live_id!(SizeRow),
                    VisKind::Measured => live_id!(MeasuredRow),
                    VisKind::Identity => live_id!(IdentityRow),
                    VisKind::BoxInset(_) => live_id!(BoxRow),
                    VisKind::FlowSpacing => live_id!(FlowRow),
                    VisKind::AlignGrid => live_id!(AlignRow),
                    VisKind::Group(_) => live_id!(GroupRow),
                    VisKind::Container => live_id!(ContainerRow),
                    VisKind::Absolute => live_id!(AbsRow),
                    VisKind::GridTracks => live_id!(GridRow),
                    VisKind::Cell => live_id!(CellRow),
                    VisKind::TweakHeader(..) | VisKind::InputsHeader(_) => live_id!(SectionRow),
                    VisKind::CascadeLevel(_) => live_id!(CascadeRow),
                    VisKind::Prop(index) | VisKind::Tweakable(index) => match self.rows[index].struct_kind {
                        StructKind::Vec2 | StructKind::Vec3 | StructKind::Vec4 => live_id!(VecRow),
                        StructKind::Inset => live_id!(InsetRow),
                        StructKind::Metrics => live_id!(MetricsRow),
                        StructKind::NoEditor => live_id!(NoEditorRow),
                        StructKind::None => match self.rows[index].kind {
                            RowKind::Num => live_id!(NumRow),
                            RowKind::Bool => live_id!(BoolRow),
                            RowKind::Color => live_id!(ColorRow),
                            RowKind::Text => live_id!(TextRow),
                            RowKind::Info => live_id!(InfoRow),
                        },
                    },
                };
                let (item, existed) = list.item_with_existed(cx, entry_id, template);
                if item.is_empty() {
                    continue;
                }
                match entry {
                    VisKind::Section(section, count, open) => {
                        let title = if self.panel_tab == PanelTab::Theme {
                            match section {
                                SectionKind::Style => "Colours",
                                SectionKind::Layout => "Spacing & sizes",
                                SectionKind::Text => "Font sizes",
                                _ => section.title(),
                            }
                        } else {
                            section.title()
                        };
                        item.child(live_id!(title)).set_text(cx, title);
                        item.child(live_id!(count))
                            .set_text(cx, &format!("{count}{}", if open { "" } else { "  +" }));
                    }
                    VisKind::More(_, count) => {
                        item.child(live_id!(title)).set_text(cx, &format!("+ show all ({count})"));
                        item.child(live_id!(count)).set_text(cx, "");
                    }
                    VisKind::TweakHeader(count, open) => {
                        item.child(live_id!(title)).set_text(cx, "Shader constants");
                        item.child(live_id!(count))
                            .set_text(cx, &format!("{count}{}", if open { "" } else { "  +" }));
                    }
                    VisKind::InputsHeader(count) => {
                        item.child(live_id!(title)).set_text(cx, "Inputs");
                        item.child(live_id!(count)).set_text(cx, &format!("{count}"));
                    }
                    VisKind::CascadeLevel(level) => {
                        if let Some(lvl) = self.cascade.get(level).cloned() {
                            let kind = cascade_icon_kind(&lvl.file);
                            let head = item.child(live_id!(head));
                            for (k, id) in [live_id!(ic_app), live_id!(ic_lib), live_id!(ic_theme), live_id!(ic_native)].into_iter().enumerate() {
                                head.child(id).set_visible(cx, k == kind);
                            }
                            let mut chip = head.child(live_id!(chip));
                            chip.child(live_id!(lbl)).set_text(cx, &format!("L{level}"));
                            let color = Self::level_color(level);
                            script_apply_eval!(cx, chip, { draw_bg +: { color: #(color) } });
                            head.child(live_id!(loc)).set_text(cx, if lvl.file.is_empty() { "native" } else { &lvl.loc });
                            let own: Vec<&str> = lvl.sets.iter().map(|(n, _)| n.as_str()).collect();
                            let over: Vec<&str> = lvl.sets.iter().filter(|(_, o)| *o).map(|(n, _)| n.as_str()).collect();
                            item.child(live_id!(sets_wrap)).set_visible(cx, !own.is_empty());
                            item.child(live_id!(overridden_wrap)).set_visible(cx, !over.is_empty());
                            item.child(live_id!(sets_wrap)).child(live_id!(sets)).set_text(cx, &if own.is_empty() { String::new() } else { format!("sets {}", own.join(" \u{00b7} ")) });
                            item.child(live_id!(overridden_wrap)).child(live_id!(overridden)).set_text(cx, &if over.is_empty() {
                                String::new()
                            } else {
                                // the closer level that wins each one
                                let mut by: Vec<String> = Vec::new();
                                for name in &over {
                                    let at = self.origin_levels.get(*name).map(|l| format!("L{l}")).unwrap_or_default();
                                    by.push(if at.is_empty() { name.to_string() } else { format!("{name} \u{2192} {at}") });
                                }
                                format!("overridden {}", by.join(" \u{00b7} "))
                            });
                        }
                    }
                    VisKind::Material(mi) => {
                        let layer = self.materials.get(mi).cloned().unwrap_or_default();
                        item.child(live_id!(name)).set_text(cx, &layer);
                        let sw_ref = item.child(live_id!(swatch_bg)).child(live_id!(swatch));
                        let sw_opt = sw_ref.borrow_mut::<TweakMaterialSwatch>();
                        if let Some(mut sw) = sw_opt {
                            sw.clip = list_viewport;
                            if sw.applied_gen != self.rows_gen || sw.layer != layer || sw.mirror_uid != self.rows_uid {
                                let widget =
                                    cx.widget_tree().widget(WidgetUid(self.rows_uid));
                                if !widget.is_empty() {
                                    sw.mirror_uid = widget.widget_uid().0;
                                    sw.mirror_primary = self.materials.first().is_some_and(|m| *m == layer);
                                    sw.mirror_layer = layer.clone();
                                    sw.applied_gen = self.rows_gen;
                                    sw.layer = layer;
                                }
                            }
                        }
                    }
                    VisKind::Identity => {
                        let sel = session().lock().unwrap().pinned.clone();
                        let (name, wanted, ty) = match sel {
                            Some(sel) => {
                                let reference = self.sel_ref(cx, sel.uid);
                                let wanted = {
                                    let mut s = session().lock().unwrap();
                                    s.load_renames();
                                    s.renames
                                        .iter()
                                        .find(|r| r.reference == reference)
                                        .map(|r| r.to.clone())
                                };
                                (tree_name_of(cx, sel.uid), wanted, sel.ty)
                            }
                            None => (String::new(), None, String::new()),
                        };
                        let field = item.child(live_id!(name_field));
                        self.identity_uid = field.widget_uid().0;
                        // A name that has been asked for but not yet carried
                        // out shows in amber: it is what the person wants the
                        // widget called, not what it is called.
                        let mut field_ref = field.clone();
                        let color: Vec4f = if wanted.is_some() {
                            vec4(1.0, 0.78, 0.29, 1.0)
                        } else {
                            vec4(0.902, 0.902, 0.902, 1.0)
                        };
                        script_apply_eval!(cx, field_ref, { draw_text +: { color: #(color) } });
                        // Never while it is being typed in.
                        if field.area() == Area::Empty || !cx.has_key_focus(field.area()) {
                            field.set_text(cx, wanted.as_deref().unwrap_or(&name));
                        }
                        item.child(live_id!(type_label)).set_text(cx, &ty);
                    }
                    VisKind::Measured => {
                        // Straight off the selection: what the layout gave,
                        // in the unit the size fields take, and — only when
                        // the screen is not 1:1, where the two differ — the
                        // device pixels it actually covers.
                        let rect = session()
                            .lock()
                            .unwrap()
                            .pinned
                            .as_ref()
                            .map(|p| p.rect)
                            .unwrap_or_default();
                        let dpi = cx.current_dpi_factor();
                        let text = if rect.size.x <= 0.0 && rect.size.y <= 0.0 {
                            String::new()
                        } else if (dpi - 1.0).abs() < 0.001 {
                            format!(
                                "measured {} \u{00d7} {} px",
                                fmt_measure(rect.size.x),
                                fmt_measure(rect.size.y)
                            )
                        } else {
                            format!(
                                "measured {} \u{00d7} {} = {} \u{00d7} {} device px",
                                fmt_measure(rect.size.x),
                                fmt_measure(rect.size.y),
                                fmt_measure((rect.size.x * dpi).round()),
                                fmt_measure((rect.size.y * dpi).round())
                            )
                        };
                        item.child(live_id!(measured)).set_text(cx, &text);
                    }
                    VisKind::Size => {
                        item.child(live_id!(name)).set_text(cx, "size");
                        let size_col = item.child(live_id!(size_col));
                        for (axis, row_id, seg_id, input_id, segs, lines, fields) in [
                            (
                                "width",
                                live_id!(w_row),
                                live_id!(w_seg),
                                live_id!(w_input),
                                [live_id!(w_fill), live_id!(w_fit), live_id!(w_fix)],
                                [live_id!(w_clamp), live_id!(w_grow), live_id!(w_basis)],
                                [live_id!(w_min), live_id!(w_max), live_id!(w_weight), live_id!(w_shrink), live_id!(w_basis_in)],
                            ),
                            (
                                "height",
                                live_id!(h_row),
                                live_id!(h_seg),
                                live_id!(h_input),
                                [live_id!(h_fill), live_id!(h_fit), live_id!(h_fix)],
                                [live_id!(h_clamp), live_id!(h_grow), live_id!(h_basis)],
                                [live_id!(h_min), live_id!(h_max), live_id!(h_weight), live_id!(h_shrink), live_id!(h_basis_in)],
                            ),
                        ] {
                            let row = size_col.child(row_id);
                            // The row is ONE value now -- `Size.Fill{..}`,
                            // `Size.Fixed(200)`, `"50%"` -- read as such.
                            let size = parse_size_text(self.row_value(axis).unwrap_or("Fit"));
                            let fixed = match &size {
                                SizeText::Fixed(v) => Some(*v),
                                _ => None,
                            };
                            // The autolayout convention: Fill spreads
                            // (arrows out), Fit hugs (arrows in), and the
                            // third is a size in the person's own words --
                            // a number of points, `50%`, `25vw`, or an
                            // expression -- which the field then shows.
                            let seg = row.child(seg_id);
                            let labels = ["\u{2194}", "\u{2192}\u{2190}", "#"];
                            let active = match size {
                                SizeText::Fill { .. } => 0,
                                SizeText::Fit => 1,
                                _ => 2,
                            };
                            for (i, seg_child) in segs.into_iter().enumerate() {
                                let btn = seg.child(seg_child);
                                btn.set_text(cx, labels[i]);
                                set_button_fill(cx, btn.clone(), i == active);
                                let chunk = match i {
                                    0 => format!("{axis}: Fill"),
                                    1 => format!("{axis}: Fit"),
                                    _ => format!(
                                        "{axis}: {}",
                                        fixed.map(fmt_f64).unwrap_or_else(|| "100".into())
                                    ),
                                };
                                self.composite_clicks.push((btn.widget_uid().0, chunk));
                            }
                            let input = row.child(input_id);
                            if input.area() == Area::Empty || !cx.has_key_focus(input.area())
                            {
                                input.set_text(cx, &size.field_text());
                            }
                            self.composite_fields
                                .push((input.widget_uid().0, axis.to_string()));
                            // The content-box clamps. A row only exists once
                            // a bound is set, so no row reads as none.
                            let clamp = size_col.child(lines[0]);
                            for (child, prop) in
                                [(fields[0], format!("min_{axis}")), (fields[1], format!("max_{axis}"))]
                            {
                                let field = clamp.child(child);
                                if field.area() == Area::Empty || !cx.has_key_focus(field.area()) {
                                    let text = self.row_value(&prop).map(bound_text).unwrap_or_default();
                                    field.set_text(cx, &text);
                                }
                                self.composite_fields.push((field.widget_uid().0, prop));
                            }
                            // A Fill's own fields, on their lines under the
                            // axis; the lines are not there for anything else.
                            let fill = match &size {
                                SizeText::Fill(fill) => Some(fill.clone()),
                                _ => None,
                            };
                            let grow = size_col.child(lines[1]);
                            let basis = size_col.child(lines[2]);
                            grow.set_visible(cx, fill.is_some());
                            basis.set_visible(cx, fill.is_some());
                            if let Some(fill) = fill {
                                for (child, key, value) in
                                    [(fields[2], "weight", fill.weight), (fields[3], "shrink", fill.shrink)]
                                {
                                    let field = grow.child(child);
                                    if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                                        input.set_value(cx, value);
                                    }
                                    self.composite_fields
                                        .push((field.widget_uid().0, format!("{axis}#{key}")));
                                }
                                let field = basis.child(fields[4]);
                                if field.area() == Area::Empty || !cx.has_key_focus(field.area()) {
                                    field.set_text(cx, &fill.basis);
                                }
                                self.composite_fields
                                    .push((field.widget_uid().0, format!("{axis}#basis")));
                            }
                        }
                        // The aspect, width over height, under both axes.
                        let field = size_col.child(live_id!(aspect_row)).child(live_id!(aspect_in));
                        if field.area() == Area::Empty || !cx.has_key_focus(field.area()) {
                            let text = self
                                .row_value("aspect")
                                .and_then(|v| v.parse::<f64>().ok())
                                .map(fmt_f64)
                                .unwrap_or_default();
                            field.set_text(cx, &text);
                        }
                        self.composite_fields.push((field.widget_uid().0, "aspect".to_string()));
                    }
                    VisKind::BoxInset(kind) => {
                        let base = kind.prop();
                        item.child(live_id!(name)).set_text(cx, base);
                        let all = self
                            .row_value(base)
                            .and_then(|v| v.parse::<f64>().ok());
                        let box_col = item.child(live_id!(box_col));
                        let legs = [
                            (live_id!(mid_row), live_id!(leg_left), "left"),
                            (live_id!(top_row), live_id!(leg_top), "top"),
                            (live_id!(mid_row), live_id!(leg_right), "right"),
                            (live_id!(bot_row), live_id!(leg_bottom), "bottom"),
                        ];
                        for (row, child, leg) in legs {
                            let prop = format!("{base}.{leg}");
                            let value = self
                                .row_value(&prop)
                                .and_then(|v| v.parse::<f64>().ok())
                                .or(all)
                                .unwrap_or(0.0);
                            let field = box_col.child(row).child(child);
                            if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                                input.set_value(cx, value);
                            }
                            self.composite_fields.push((field.widget_uid().0, prop));
                        }
                        let link = item.child(live_id!(link));
                        let link_index = (kind == BoxKind::Padding) as usize;
                        if let Some(mut check) = link.borrow_mut::<CheckBox>() {
                            check.set_active(cx, self.box_link[link_index], Animate::No);
                        }
                        self.box_link_uids[link_index] = link.widget_uid().0;
                    }
                    VisKind::FlowSpacing => {
                        item.child(live_id!(name)).set_text(cx, "flow");
                        // The row is one value -- `Flow.Right{wrap: true ..}`
                        // -- and every button writes the whole of it back.
                        let flow = parse_flow_text(self.row_value("flow").unwrap_or(""));
                        let col = item.child(live_id!(flow_col));
                        let dir_row = col.child(live_id!(dir_row));
                        let seg = dir_row.child(live_id!(flow_seg));
                        let dirs = [
                            (live_id!(f_right), "\u{2192}", FlowDir::Right),
                            (live_id!(f_down), "\u{2193}", FlowDir::Down),
                            (live_id!(f_over), "stack", FlowDir::Overlay),
                        ];
                        for (child, label, dir) in dirs {
                            let btn = seg.child(child);
                            btn.set_text(cx, label);
                            set_button_fill(cx, btn.clone(), flow.dir == dir);
                            self.composite_clicks
                                .push((btn.widget_uid().0, flow.with_dir(dir).chunk()));
                        }
                        let wrap_btn = dir_row.child(live_id!(f_wrap));
                        wrap_btn.set_text(cx, "wrap");
                        set_button_fill(cx, wrap_btn.clone(), flow.wraps());
                        self.composite_clicks
                            .push((wrap_btn.widget_uid().0, flow.toggled_wrap().chunk()));
                        // How a row lines up only means anything left to
                        // right; the segment is not shown otherwise.
                        let ra_seg = dir_row.child(live_id!(ra_seg));
                        ra_seg.set_visible(cx, flow.dir == FlowDir::Right);
                        let aligns = [
                            (live_id!(ra_top), "\u{2191}", RowAlignText::Top),
                            (live_id!(ra_mid), "\u{2195}", RowAlignText::Center),
                            (live_id!(ra_bottom), "\u{2193}", RowAlignText::Bottom),
                        ];
                        for (child, label, row_align) in aligns {
                            let btn = ra_seg.child(child);
                            btn.set_text(cx, label);
                            set_button_fill(
                                cx,
                                btn.clone(),
                                flow.dir == FlowDir::Right && flow.row_align == row_align,
                            );
                            self.composite_clicks
                                .push((btn.widget_uid().0, flow.with_row_align(row_align).chunk()));
                        }
                        // The gaps: between children, and between wrapped
                        // rows. A Label flows its text but has no spacing,
                        // so the line only shows where there is one to set.
                        let gap_row = col.child(live_id!(gap_row));
                        gap_row.set_visible(cx, self.row_index("spacing").is_some());
                        let field = gap_row.child(live_id!(spacing_input));
                        if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                            let v = self
                                .row_value("spacing")
                                .and_then(|v| v.parse::<f64>().ok())
                                .unwrap_or(0.0);
                            input.set_value(cx, v);
                        }
                        self.composite_fields
                            .push((field.widget_uid().0, "spacing".into()));
                        let wrap_box = gap_row.child(live_id!(wrap_box));
                        wrap_box.set_visible(
                            cx,
                            flow.wraps() && self.row_index("wrap_spacing").is_some(),
                        );
                        let field = wrap_box.child(live_id!(wrap_input));
                        if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                            let v = self
                                .row_value("wrap_spacing")
                                .and_then(|v| v.parse::<f64>().ok())
                                .unwrap_or(0.0);
                            input.set_value(cx, v);
                        }
                        self.composite_fields
                            .push((field.widget_uid().0, "wrap_spacing".into()));
                    }
                    VisKind::AlignGrid => if self.sel_is_grid {
                        // A Grid aligns each child inside its cell: along
                        // the columns and along the rows, stretch included.
                        item.child(live_id!(name)).set_text(cx, "cells");
                        let col = item.child(live_id!(align_col));
                        col.child(live_id!(space_row)).set_visible(cx, false);
                        for (row_id, seg_id, axis_id, prop, arrow, ids) in [
                            (
                                live_id!(just_row),
                                live_id!(just_seg),
                                live_id!(just_axis),
                                "justify_items",
                                "",
                                [live_id!(j_stretch), live_id!(j_start), live_id!(j_mid), live_id!(j_end)],
                            ),
                            (
                                live_id!(cross_row),
                                live_id!(cross_seg),
                                live_id!(cross_axis),
                                "align_items",
                                "",
                                [live_id!(c_stretch), live_id!(c_start), live_id!(c_mid), live_id!(c_end)],
                            ),
                        ] {
                            let row = col.child(row_id);
                            row.child(axis_id).set_text(cx, arrow);
                            let current = self
                                .row_value(prop)
                                .map(|v| v.rsplit('.').next().unwrap_or("").to_string())
                                .unwrap_or_else(|| "Stretch".to_string());
                            let seg = row.child(seg_id);
                            for (child, label, variant) in [
                                (ids[0], "stretch", "Stretch"),
                                (ids[1], "start", "Start"),
                                (ids[2], "centre", "Center"),
                                (ids[3], "end", "End"),
                            ] {
                                let btn = seg.child(child);
                                btn.set_visible(cx, true);
                                btn.set_text(cx, label);
                                set_button_fill(cx, btn.clone(), current == variant);
                                self.composite_clicks
                                    .push((btn.widget_uid().0, format!("{prop}: CellAlign.{variant}")));
                            }
                        }
                    } else {
                            item.child(live_id!(name)).set_text(cx, "align");
                            let ax = self
                                .row_value("align.x")
                                .and_then(|v| v.parse::<f64>().ok())
                                .unwrap_or(0.0);
                            let ay = self
                                .row_value("align.y")
                                .and_then(|v| v.parse::<f64>().ok())
                                .unwrap_or(0.0);
                            // Which axis is along the flow and which across
                            // follows the direction; the labels carry the arrow.
                            let dir = parse_flow_text(self.row_value("flow").unwrap_or("")).dir;
                            let down = dir == FlowDir::Down;
                            let (main, cross) = if down { (ay, ax) } else { (ax, ay) };
                            let distribute = self
                                .row_value("distribute")
                                .map(|v| v.rsplit('.').next().unwrap_or("").to_string())
                                .unwrap_or_else(|| "Start".to_string());
                            let col = item.child(live_id!(align_col));
                            col.child(live_id!(space_row)).set_visible(cx, true);
                            let just_row = col.child(live_id!(just_row));
                            just_row
                                .child(live_id!(just_axis))
                                .set_text(cx, if down { "\u{2195}" } else { "\u{2194}" });
                            let seg = just_row.child(live_id!(just_seg));
                            seg.child(live_id!(j_stretch)).set_visible(cx, false);
                            for (child, label, value) in [
                                (live_id!(j_start), "start", 0.0),
                                (live_id!(j_mid), "centre", 0.5),
                                (live_id!(j_end), "end", 1.0),
                            ] {
                                let btn = seg.child(child);
                                btn.set_text(cx, label);
                                set_button_fill(
                                    cx,
                                    btn.clone(),
                                    distribute == "Start" && (main - value).abs() < 0.25,
                                );
                                let (x, y) = if down { (ax, value) } else { (value, ay) };
                                self.composite_clicks.push((
                                    btn.widget_uid().0,
                                    format!(
                                        "align: Align{{x: {} y: {}}} distribute: Distribute.Start",
                                        fmt_f64(x),
                                        fmt_f64(y)
                                    ),
                                ));
                            }
                            let seg = col.child(live_id!(space_row)).child(live_id!(space_seg));
                            for (child, label, variant) in [
                                (live_id!(s_between), "between", "SpaceBetween"),
                                (live_id!(s_around), "around", "SpaceAround"),
                                (live_id!(s_evenly), "evenly", "SpaceEvenly"),
                            ] {
                                let btn = seg.child(child);
                                btn.set_text(cx, label);
                                set_button_fill(cx, btn.clone(), distribute == variant);
                                self.composite_clicks
                                    .push((btn.widget_uid().0, format!("distribute: Distribute.{variant}")));
                            }
                            let cross_row = col.child(live_id!(cross_row));
                            cross_row
                                .child(live_id!(cross_axis))
                                .set_text(cx, if down { "\u{2194}" } else { "\u{2195}" });
                            let seg = cross_row.child(live_id!(cross_seg));
                            seg.child(live_id!(c_stretch)).set_visible(cx, false);
                            for (child, label, value) in [
                                (live_id!(c_start), "start", 0.0),
                                (live_id!(c_mid), "centre", 0.5),
                                (live_id!(c_end), "end", 1.0),
                            ] {
                                let btn = seg.child(child);
                                btn.set_text(cx, label);
                                set_button_fill(cx, btn.clone(), (cross - value).abs() < 0.25);
                                let (x, y) = if down { (value, ay) } else { (ax, value) };
                                self.composite_clicks.push((
                                    btn.widget_uid().0,
                                    format!("align: Align{{x: {} y: {}}}", fmt_f64(x), fmt_f64(y)),
                                ));
                            }
                        }
                    VisKind::GridTracks => {
                        item.child(live_id!(name)).set_text(cx, "grid");
                        let col = item.child(live_id!(grid_col));
                        // The tracks, as the CSS they were written in.
                        for (row_id, input_id, prop) in [
                            (live_id!(cols_row), live_id!(cols_in), "columns"),
                            (live_id!(rows_row), live_id!(rows_in), "rows"),
                        ] {
                            let field = col.child(row_id).child(input_id);
                            if field.area() == Area::Empty || !cx.has_key_focus(field.area()) {
                                field.set_text(cx, &quoted_list_text(self.row_value(prop).unwrap_or(""), " "));
                            }
                            self.composite_fields.push((field.widget_uid().0, prop.to_string()));
                        }
                        let gaps = col.child(live_id!(gaps_row));
                        for (child, prop) in
                            [(live_id!(gap_col), "column_gap"), (live_id!(gap_row_in), "row_gap")]
                        {
                            let field = gaps.child(child);
                            if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                                let v = self
                                    .row_value(prop)
                                    .and_then(|v| v.parse::<f64>().ok())
                                    .unwrap_or(0.0);
                                input.set_value(cx, v);
                            }
                            self.composite_fields.push((field.widget_uid().0, prop.to_string()));
                        }
                        // Which way the cells without a place are filled in.
                        let auto = self
                            .row_value("auto_flow")
                            .map(|v| v.rsplit('.').next().unwrap_or("").to_string())
                            .unwrap_or_else(|| "Row".to_string());
                        let seg = col.child(live_id!(fill_row)).child(live_id!(fill_seg));
                        for (child, label, variant) in [
                            (live_id!(f_rows), "by row", "Row"),
                            (live_id!(f_cols), "by column", "Column"),
                        ] {
                            let btn = seg.child(child);
                            btn.set_text(cx, label);
                            set_button_fill(cx, btn.clone(), auto == variant);
                            self.composite_clicks
                                .push((btn.widget_uid().0, format!("auto_flow: AutoFlow.{variant}")));
                        }
                        let field = col.child(live_id!(areas_row)).child(live_id!(areas_in));
                        if field.area() == Area::Empty || !cx.has_key_focus(field.area()) {
                            field.set_text(cx, &quoted_list_text(self.row_value("areas").unwrap_or(""), " / "));
                        }
                        self.composite_fields.push((field.widget_uid().0, "areas".to_string()));
                    }
                    VisKind::Cell => {
                        item.child(live_id!(name)).set_text(cx, "cell");
                        let cell = self.cell_text();
                        let col = item.child(live_id!(cell_col));
                        for (row_id, child, key, value) in [
                            (live_id!(place_row), live_id!(cell_c), "col", cell.col),
                            (live_id!(place_row), live_id!(cell_r), "row", cell.row),
                            (live_id!(span_row), live_id!(cell_cs), "col_span", cell.col_span),
                            (live_id!(span_row), live_id!(cell_rs), "row_span", cell.row_span),
                        ] {
                            let field = col.child(row_id).child(child);
                            if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                                input.set_value(cx, value as f64);
                            }
                            self.composite_fields
                                .push((field.widget_uid().0, format!("cell#{key}")));
                        }
                        let field = col.child(live_id!(area_row)).child(live_id!(cell_area));
                        if field.area() == Area::Empty || !cx.has_key_focus(field.area()) {
                            field.set_text(cx, &cell.area);
                        }
                        self.composite_fields
                            .push((field.widget_uid().0, "cell#area".to_string()));
                    }
                    VisKind::Group(kind) => {
                        item.child(live_id!(title)).set_text(
                            cx,
                            match kind {
                                GroupKind::Item => "in its parent",
                                GroupKind::Container => "its children",
                            },
                        );
                    }
                    VisKind::Container => {
                        item.child(live_id!(name)).set_text(cx, "layout");
                        let sel = session().lock().unwrap().pinned.clone();
                        let (ty, wanted) = match sel {
                            Some(sel) => {
                                let reference = self.sel_ref(cx, sel.uid);
                                let wanted = {
                                    let mut s = session().lock().unwrap();
                                    s.load_converts();
                                    s.converts
                                        .iter()
                                        .find(|c| c.reference == reference)
                                        .map(|c| c.to.clone())
                                };
                                (sel.ty, wanted)
                            }
                            None => (String::new(), None),
                        };
                        let is_grid = ty == "Grid";
                        let col = item.child(live_id!(ctr_col));
                        let mode_row = col.child(live_id!(mode_row));
                        mode_row
                            .child(live_id!(mode_label))
                            .set_text(cx, if is_grid { "grid" } else { "flex" });
                        // The ask is a toggle: lit while it stands, and a
                        // second press takes it back.
                        let btn = mode_row.child(live_id!(convert));
                        btn.set_text(cx, if is_grid { "make flex" } else { "make grid" });
                        set_button_fill(cx, btn.clone(), wanted.is_some());
                        self.convert_uid = btn.widget_uid().0;
                        let field = col.child(live_id!(name_row)).child(live_id!(ctr_name));
                        if field.area() == Area::Empty || !cx.has_key_focus(field.area()) {
                            let text = self
                                .row_value("container_id")
                                .map(|v| v.trim_start_matches('@'))
                                .filter(|v| *v != "-")
                                .unwrap_or("");
                            field.set_text(cx, text);
                        }
                        self.composite_fields
                            .push((field.widget_uid().0, "container_id".to_string()));
                    }
                    VisKind::Absolute => {
                        item.child(live_id!(name)).set_text(cx, "position");
                        let pos = self.row_value("abs_pos").and_then(parse_vec2_text);
                        let col = item.child(live_id!(abs_col));
                        let check = col.child(live_id!(abs_check));
                        if let Some(mut check) = check.borrow_mut::<CheckBox>() {
                            check.set_active(cx, pos.is_some(), Animate::No);
                        }
                        self.abs_uid = check.widget_uid().0;
                        let xy = col.child(live_id!(abs_xy));
                        xy.set_visible(cx, pos.is_some());
                        if let Some((x, y)) = pos {
                            for (child, key, value) in
                                [(live_id!(abs_x), "x", x), (live_id!(abs_y), "y", y)]
                            {
                                let field = xy.child(child);
                                if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                                    input.set_value(cx, value);
                                }
                                self.composite_fields
                                    .push((field.widget_uid().0, format!("abs_pos#{key}")));
                            }
                        }
                    }
                    VisKind::Prop(index) | VisKind::Tweakable(index) => {
                        let as_tweakable = matches!(entry, VisKind::Tweakable(_));
                        let name = item.child(live_id!(name));
                        name.set_text(cx, &self.rows[index].prop);
                        let _ = &name;
                        // The changed-indicator: a resettable row's label
                        // reads brighter (double-click it to reset).
                        // Cascade rows: the level header takes its level's
                        // color (the same palette the origin dots use).
                        let cascade_level = if self.rows[index].section == SectionKind::Cascade {
                            let prop = &self.rows[index].prop;
                            prop.strip_prefix('L')
                                .and_then(|rest| rest.split(' ').next())
                                .and_then(|n| n.parse::<usize>().ok())
                        } else {
                            None
                        };
                        if let Some(mut label) = name.borrow_mut::<Label>() {
                            label.draw_text.color = if let Some(level) = cascade_level {
                                Self::level_color(level)
                            } else if self.rows[index].changed {
                                vec4(1.0, 0.78, 0.42, 1.0)
                            } else if self.row_docs.contains_key(&self.rows[index].prop) {
                                // annotated: the author meant this one to be tweaked
                                vec4(0.86, 0.80, 0.58, 1.0)
                            } else if self.rows[index].set {
                                // set at the instance level
                                vec4(0.72, 0.72, 0.72, 1.0)
                            } else {
                                // inherited from a prototype level
                                vec4(0.48, 0.48, 0.48, 1.0)
                            };
                        }
                        // The origin dot: colored by the closest proto
                        // level that sets this prop; click opens the
                        // cascade scrolled to that level.
                        let origin = item.child(live_id!(origin));
                        if !origin.is_empty() {
                            let first = self.rows[index]
                                .prop
                                .split('.')
                                .next()
                                .unwrap_or("")
                                .to_string();
                            match self.origin_levels.get(&first) {
                                Some(&level) => {
                                    origin.set_text(cx, "\u{25cf}");
                                    if let Some(mut label) = origin.borrow_mut::<Label>() {
                                        label.draw_text.color = Self::level_color(level);
                                    }
                                }
                                None => origin.set_text(cx, ""),
                            }
                        }
                        let sk = self.rows[index].struct_kind;
                        if sk != StructKind::None {
                            match sk {
                                StructKind::Vec2 | StructKind::Vec3 | StructKind::Vec4 => {
                                    let n = match sk {
                                        StructKind::Vec2 => 2,
                                        StructKind::Vec3 => 3,
                                        _ => 4,
                                    };
                                    for (c, cid) in [live_id!(vx), live_id!(vy), live_id!(vz), live_id!(vw)]
                                        .into_iter()
                                        .enumerate()
                                    {
                                        let f = item.child(cid);
                                        if c == 2 {
                                            item.child(live_id!(vz_wrap)).set_visible(cx, c < n);
                                        } else if c == 3 {
                                            item.child(live_id!(vw_wrap)).set_visible(cx, c < n);
                                        }
                                        if c < n {
                                            self.rows[index].comp_uids.push(f.widget_uid().0);
                                            let v = self.rows[index].comp_vals.get(c).copied().unwrap_or(0.0);
                                            let input_opt = f.borrow_mut::<FabValueInput>();
                                            if let Some(mut input) = input_opt {
                                                input.set_value(cx, v);
                                            }
                                        }
                                    }
                                }
                                StructKind::Inset => {
                                    for (c, cid) in [live_id!(il), live_id!(it), live_id!(ir), live_id!(ib)]
                                        .into_iter()
                                        .enumerate()
                                    {
                                        let f = item.child(cid);
                                        self.rows[index].comp_uids.push(f.widget_uid().0);
                                        let v = self.rows[index].comp_vals.get(c).copied().unwrap_or(0.0);
                                        let input_opt = f.borrow_mut::<FabValueInput>();
                                        if let Some(mut input) = input_opt {
                                            input.set_value(cx, v);
                                        }
                                    }
                                }
                                StructKind::Metrics => {
                                    for (c, cid) in [live_id!(m0), live_id!(m1), live_id!(m2)]
                                        .into_iter()
                                        .enumerate()
                                    {
                                        let f = item.child(cid);
                                        self.rows[index].comp_uids.push(f.widget_uid().0);
                                        let v = self.rows[index].comp_vals.get(c).copied().unwrap_or(0.0);
                                        let input_opt = f.borrow_mut::<FabValueInput>();
                                        if let Some(mut input) = input_opt {
                                            input.set_value(cx, v);
                                        }
                                    }
                                }
                                StructKind::NoEditor | StructKind::None => {}
                            }
                        } else {
                        let field = item.child(live_id!(value));
                        if as_tweakable {
                            self.rows[index].alt_uids.push((field.widget_uid().0, false));
                        } else {
                            self.rows[index].field_uid = field.widget_uid().0;
                        }
                        match self.rows[index].kind {
                            RowKind::Num => {
                                if let Some(mut input) = field.borrow_mut::<FabValueInput>() {
                                    // The annotation channel drives the
                                    // scrubber: `/**name 0..24 step 0.5*/`
                                    // becomes bounds + granularity.
                                    if let Some(doc) = self.row_docs.get(&self.rows[index].prop)
                                    {
                                        let hint = parse_doc_hint(doc);
                                        input.set_hint(hint.min, hint.max, hint.step);
                                    }
                                    if let Ok(v) = self.rows[index].value.parse::<f64>() {
                                        input.set_value(cx, v);
                                    }
                                }
                            }
                            RowKind::Bool => {
                                if let Some(mut check) = field.borrow_mut::<CheckBox>() {
                                    check.set_active(
                                        cx,
                                        self.rows[index].value == "true",
                                        Animate::No,
                                    );
                                }
                            }
                            RowKind::Text => {
                                let editing = self
                                    .text_edit_origin
                                    .as_ref()
                                    .is_some_and(|(row, _)| *row == index);
                                if !editing
                                    && (!existed
                                        || field.area() == Area::Empty
                                        || !cx.has_key_focus(field.area()))
                                {
                                    field.set_text(cx, &self.rows[index].value);
                                }
                            }
                            RowKind::Info => {
                                field.set_text(cx, &self.rows[index].value);
                            }
                            RowKind::Color => {
                                if field.area() == Area::Empty
                                    || !cx.has_key_focus(field.area())
                                {
                                    field.set_text(cx, &self.rows[index].value);
                                }
                                let swatch = item.child(live_id!(swatch));
                                if as_tweakable {
                                    self.rows[index].alt_uids.push((swatch.widget_uid().0, true));
                                } else {
                                    self.rows[index].swatch_uid = swatch.widget_uid().0;
                                }
                                let rgba = parse_hex(&self.rows[index].value);
                                // Theme mapping: a value that IS a theme
                                // colour names it, and one click makes the
                                // property say `theme.color_x` in splash.
                                // (a theme row IS its colour: no chip there)
                                let matched = if self.panel_tab == PanelTab::Theme {
                                    None
                                } else {
                                    rgba.and_then(|(c, _)| {
                                        let packed = packed_of(c);
                                        self.theme_colors.iter().find(|(_, tc, _)| *tc == packed).map(|(n, _, _)| n.clone())
                                    })
                                };
                                let wrap = item.child(live_id!(tname_wrap));
                                wrap.set_visible(cx, matched.is_some());
                                if let Some(name) = &matched {
                                    let btn = wrap.child(live_id!(tname));
                                    btn.set_text(cx, &format!("\u{2248} theme.{name}"));
                                    self.rows[index].theme_uid = btn.widget_uid().0;
                                }
                                self.rows[index].theme_match = matched;
                                {
                                    if let Some(mut pick) =
                                        swatch.borrow_mut::<FabColorPick>()
                                    {
                                        if !pick.is_open() {
                                            if let Some((rgba, _)) = rgba {
                                                pick.set_rgba(cx, rgba);
                                            }
                                        } else {
                                            let rect = pick.popover_rect();
                                            if rect.size.x > 0.0 {
                                                self.open_popup = Some(rect);
                                                session().lock().unwrap().popup = Some(rect);
                                            }
                                        }
                                    }
                                }
                                drop(swatch);
                            }
                        }
                        }
                    }
                }
                item.draw_all(cx, &mut Scope::empty());
                // The colour popover's rect is known once it has drawn:
                // input priority for the popup, and /tweak/state's `popup`.
                if let Some(pick) = item.child(live_id!(swatch)).borrow::<FabColorPick>() {
                    let rect = pick.popover_rect();
                    if rect.size.x > 0.0 {
                        self.open_popup = Some(rect);
                        session().lock().unwrap().popup = Some(rect);
                    }
                }
                visible_rects.push(VisRow {
                    kind: entry,
                    item: item.clone(),
                });
            }
        }
        self.visible = visible_rects;
    }

    /// Apply one sidebar-originated chunk to the selection (same path the
    /// AI uses, same diff log; TWEAK log lines throttle to one per pause).
    /// One small well per animator track of the pinned widget, under the
    /// main well: the same byte-copied draw call, posed by that track's
    /// off/on apply values and cycled on the frame clock. Every scrub or
    /// fn edit shows in all of them at once — they mirror the live call.
    fn refresh_state_swatches(&mut self, cx: &mut Cx, col: &WidgetRef) {
        let layer = self.vibe_layer.clone().unwrap_or_else(|| "draw_bg".to_string());
        let row = col.child(live_id!(states_row));
        let widget = cx.widget_tree().widget(WidgetUid(self.rows_uid));
        if widget.is_empty() {
            row.set_visible(cx, false);
            self.state_tracks.clear();
            return;
        }
        if self.states_gen != self.rows_gen || self.states_uid != self.rows_uid || self.states_layer != layer {
            self.state_tracks = animator_tracks(cx, &widget, &layer);
            self.states_gen = self.rows_gen;
            if self.states_uid != self.rows_uid {
                // A new selection starts its cycle at the off pose.
                self.states_t0 = cx.seconds_since_app_start();
            }
            self.states_uid = self.rows_uid;
            self.states_layer = layer.clone();
            session().lock().unwrap().state_names = self.state_tracks.iter().map(|t| t.group.clone()).collect();
        }
        row.set_visible(cx, !self.state_tracks.is_empty());
        let primary = self.materials.first().is_some_and(|m| *m == layer);
        let slots = [live_id!(st0), live_id!(st1), live_id!(st2), live_id!(st3), live_id!(st4), live_id!(st5)];
        for (i, slot) in slots.into_iter().enumerate() {
            let item = row.child(slot);
            match self.state_tracks.get(i) {
                Some(track) => {
                    item.set_visible(cx, true);
                    let mut label = format!(
                        "{} \u{00b7} in {}s, out {}s",
                        track.group,
                        fmt_f64(play_duration(track.on_play)),
                        fmt_f64(play_duration(track.off_play))
                    );
                    if let Some(doc) = self.row_docs.get(&format!("animator.{}", track.group)) {
                        let first = doc.lines().next().unwrap_or("").trim();
                        if !first.is_empty() {
                            label.push_str(" \u{00b7} ");
                            label.push_str(first);
                        }
                    }
                    item.child(live_id!(lbl)).set_text(cx, &label);
                    if let Some(mut sw) = item.child(live_id!(sw)).borrow_mut::<TweakMaterialSwatch>() {
                        sw.clip = self.swatch_clip;
                        sw.mirror_uid = self.rows_uid;
                        sw.mirror_primary = primary;
                        sw.mirror_layer = layer.clone();
                        sw.layer = layer.clone();
                        sw.applied_gen = self.rows_gen;
                        sw.state = Some(track.clone());
                    }
                }
                None => item.set_visible(cx, false),
            }
        }
        let pause = row.child(live_id!(states_pause));
        self.states_pause_uid = pause.widget_uid().0;
        pause.set_text(cx, if self.states_paused { "play" } else { "pause" });
        if !self.state_tracks.is_empty() {
            self.states_frame = cx.new_next_frame();
        }
    }

    /// One frame of the state swatches' cycle.
    fn states_tick(&mut self, cx: &mut Cx) {
        if self.state_tracks.is_empty() || !tweak_is_on() {
            return;
        }
        let Some(sidebar) = self.sidebar.clone() else { return };
        let col = sidebar.child(live_id!(shader_col));
        if !col.visible() {
            return;
        }
        let lock = session().lock().unwrap().states_lock;
        let paused = self.states_paused || self.states_hover;
        let t = cx.seconds_since_app_start() - self.states_t0;
        let row = col.child(live_id!(states_row));
        let slots = [live_id!(st0), live_id!(st1), live_id!(st2), live_id!(st3), live_id!(st4), live_id!(st5)];
        let mut moved = false;
        for (i, slot) in slots.into_iter().enumerate() {
            let Some(track) = self.state_tracks.get(i) else { break };
            let mix = match lock {
                Some(v) => v as f32,
                None if paused => continue,
                None => track_mix(track, t),
            };
            if let Some(mut sw) = row.child(slot).child(live_id!(sw)).borrow_mut::<TweakMaterialSwatch>() {
                if (sw.mix - mix).abs() > 1.0e-4 {
                    sw.mix = mix;
                    moved = true;
                }
            }
        }
        if moved {
            self.redraw_sidebar(cx);
        }
        if lock.is_none() && !paused {
            self.states_frame = cx.new_next_frame();
        }
    }

    /// Hovering an "≈ theme.color_x" chip pulses that colour everywhere it
    /// is drawn, live — leave restores by redrawing all (the ordinary draw
    /// path rebuilds every buffer; the pulse never touches the ledger).
    fn pulse_hover(&mut self, cx: &mut Cx, abs: Vec2d) {
        let mut over: Option<u32> = None;
        if tweak_is_on() {
            for row in &self.rows {
                if row.theme_uid == 0 {
                    continue;
                }
                let Some(name) = &row.theme_match else { continue };
                let rect = cx
                    .widget_tree()
                    .widget(WidgetUid(row.theme_uid))
                    .area()
                    .clipped_rect(cx);
                if rect.size.x > 0.0 && rect.contains(abs) {
                    over = self.theme_colors.iter().find(|(n, _, _)| n == name).map(|(_, c, _)| *c);
                    break;
                }
            }
        }
        // A remotely pinned pulse ignores the pointer until it is cleared.
        if self.pulse_pinned && over.is_none() {
            return;
        }
        self.set_pulse(cx, over);
    }

    /// Start pulsing `over` app-wide, hop to it from another colour, or
    /// (None) restore the true colour everywhere. Never touches the ledger.
    fn set_pulse(&mut self, cx: &mut Cx, over: Option<u32>) {
        match (over, self.pulse) {
            (Some(c), Some((p, _))) if c == p => {}
            (Some(c), _) => {
                // Hopping between chips restores the old colour first.
                self.pulse_end(cx);
                self.pulse = Some((c, cx.seconds_since_app_start()));
                self.pulse_ticks = 0;
                session().lock().unwrap().pulse = Some(PulseState::new(c));
                hook_sync(cx);
                self.pulse_frame = cx.new_next_frame();
                log!("TWEAK pulse on #{c:08x}");
            }
            (None, Some(_)) => {
                self.pulse_end(cx);
                log!("TWEAK pulse off — restore");
            }
            (None, None) => {}
        }
    }

    /// The theme palette for a colour popover's strip: greys first (by
    /// lightness), then by hue and lightness, so related colours sit
    /// together and the name under the strip says which one is which.
    fn palette_entries(&self) -> Vec<(String, [f32; 4])> {
        let mut entries: Vec<(String, [f32; 4], (u8, u16, u16))> = self
            .theme_colors
            .iter()
            .map(|(name, c, _)| {
                let rgba = [
                    ((c >> 24) & 0xff) as f32 / 255.0,
                    ((c >> 16) & 0xff) as f32 / 255.0,
                    ((c >> 8) & 0xff) as f32 / 255.0,
                    (c & 0xff) as f32 / 255.0,
                ];
                let [h, s, v] = rgb_to_hsv(rgba[0], rgba[1], rgba[2]);
                let key = if s < 0.1 {
                    (0u8, (v * 1000.0) as u16, (rgba[3] * 1000.0) as u16)
                } else {
                    (1u8, (h * 12.0) as u16, (v * 1000.0) as u16)
                };
                (name.clone(), rgba, key)
            })
            .collect();
        entries.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
        entries.into_iter().map(|(n, c, _)| (n, c)).collect()
    }

    /// Stop the pulse: true colours back into every slot it wrote, the
    /// hook down, and a redraw of everything for good measure.
    fn pulse_end(&mut self, cx: &mut Cx) {
        self.pulse = None;
        let st = session().lock().unwrap().pulse.take();
        hook_sync(cx);
        if let Some(st) = st {
            pulse_restore(cx, &st);
            cx.redraw_all();
        }
    }

    /// Resting the pointer on the state swatches pauses them.
    fn states_hover(&mut self, cx: &mut Cx, abs: Vec2d) {
        if self.state_tracks.is_empty() {
            return;
        }
        let Some(sidebar) = self.sidebar.as_ref() else { return };
        let row = sidebar.child(live_id!(shader_col)).child(live_id!(states_row));
        let rect = row.area().clipped_rect(cx);
        let over = rect.size.x > 0.0 && rect.contains(abs) && tweak_is_on();
        if over != self.states_hover {
            self.states_hover = over;
            if !over {
                self.states_frame = cx.new_next_frame();
            }
        }
    }

    /// The terse doc line under the layer name shows the whole doc while
    /// the pointer rests on it.
    fn doc_tip_hover(&mut self, cx: &mut Cx, abs: Vec2d) {
        let Some(sidebar) = self.sidebar.as_ref() else { return };
        let col = sidebar.child(live_id!(shader_col));
        let over = if self.shader_doc_full.is_empty() || !tweak_is_on() {
            false
        } else {
            // A label's area is its first glyph: the union is the line.
            let rect = col.child(live_id!(shader_doc)).area().clipped_rect_union(cx);
            rect.size.x > 0.0 && rect.contains(abs)
        };
        if over == self.doc_tip_shown {
            return;
        }
        self.doc_tip_shown = over;
        let tip = col.child(live_id!(doc_tip));
        let Some(mut tip) = tip.borrow_mut::<Tooltip>() else { return };
        if over {
            let rect = col.child(live_id!(shader_doc)).area().clipped_rect_union(cx);
            let pos = dvec2(rect.pos.x, rect.pos.y + rect.size.y + 2.0);
            tip.show_with_options(cx, pos, &self.shader_doc_full);
        } else {
            tip.hide(cx);
        }
    }

    /// The scope buttons explain themselves under the pointer.
    ///
    /// Which one is under it decides what is said: the two scopes differ in
    /// who else takes the edit AND in which file it lands in, and a single
    /// line covering both was the reason the old standing text had to be
    /// truncated to fit.
    /// The panel's own controls explain themselves on hover, through the
    /// scope buttons' Tooltip: one bubble, one look. Called from the pointer
    /// intercept on every move over the panel, after `scope_tip_hover`,
    /// which owns the bubble while the pointer is on a scope button and
    /// leaves it alone otherwise.
    fn chrome_tip_hover(&mut self, cx: &mut Cx, abs: Vec2d) {
        if self.scope_tip_shown != 0 {
            return;
        }
        let Some(sidebar) = self.sidebar.as_ref() else { return };
        let found = if tweak_is_on() { self.chrome_doc(cx, abs) } else { None };
        let text = found.as_ref().map(|(_, t)| t.clone()).unwrap_or_default();
        if text == self.chrome_tip_shown {
            return;
        }
        let tip = sidebar
            .child(live_id!(ident_footer))
            .child(live_id!(scope_row))
            .child(live_id!(scope_tip));
        // Measure what is on screen before it changes, so the next show can
        // place itself against a real height instead of the fallback.
        let measured = tip.child(live_id!(content)).area().clipped_rect(cx);
        if measured.size.y > 0.0 {
            self.scope_tip_size = measured.size;
        }
        self.chrome_tip_shown = text.clone();
        let Some(mut tip) = tip.borrow_mut::<Tooltip>() else { return };
        let Some((rect, _)) = found else {
            tip.hide(cx);
            return;
        };
        // Above the control, or below it for the two rows at the very top,
        // where "above" is off the window. Kept inside the window's right
        // edge, which is the band's.
        let size = self.scope_tip_size;
        let window_x = self.band.pos.x + self.band.size.x;
        let y = if rect.pos.y < 44.0 {
            rect.pos.y + rect.size.y + 4.0
        } else {
            rect.pos.y - size.y - 4.0
        };
        let x = rect.pos.x.min(window_x - size.x - 6.0).max(4.0);
        tip.show_with_options(cx, dvec2(x, y), &text);
    }

    fn scope_tip_hover(&mut self, cx: &mut Cx, abs: Vec2d) {
        let Some(sidebar) = self.sidebar.as_ref() else { return };
        let row = sidebar
            .child(live_id!(ident_footer))
            .child(live_id!(scope_row));
        let line = row.child(live_id!(scope_line));
        let over = if !tweak_is_on() {
            0
        } else {
            let hit = |id: WidgetRef| {
                let rect = id.area().clipped_rect(cx);
                rect.size.x > 0.0 && rect.contains(abs)
            };
            if hit(line.child(live_id!(scope_this))) {
                1
            } else if hit(line.child(live_id!(scope_all))) {
                2
            } else if hit(line.child(live_id!(scope_isolated))) {
                3
            } else {
                0
            }
        };
        if over == self.scope_tip_shown {
            return;
        }
        self.scope_tip_shown = over;
        let tip = row.child(live_id!(scope_tip));
        // Measure what is on screen before hiding it, so the next show can
        // place itself against a real height instead of the fallback.
        let measured = tip.child(live_id!(content)).area().clipped_rect(cx);
        if measured.size.y > 0.0 {
            self.scope_tip_size = measured.size;
        }
        let ty = session()
            .lock()
            .unwrap()
            .pinned
            .as_ref()
            .map(|p| p.ty.clone())
            .unwrap_or_else(|| "widget".to_string());
        let isolating = self.tree_isolate && self.isolate_uid != 0;
        let confined = isolating && !session().lock().unwrap().scope_unconfined;
        // The band's right edge IS the window's: it is laid out from the
        // pass width every sidebar draw.
        let window_x = self.band.pos.x + self.band.size.x;
        let size = self.scope_tip_size;
        let place = |rect: Rect| {
            dvec2(
                rect.pos.x.min(window_x - size.x - 6.0).max(4.0),
                rect.pos.y - size.y - 4.0,
            )
        };
        let Some(mut tip) = tip.borrow_mut::<Tooltip>() else { return };
        match over {
            1 => {
                let rect = line.child(live_id!(scope_this)).area().clipped_rect(cx);
                tip.show_with_options(
                    cx,
                    place(rect),
                    "this: this instance, and anything built from the same \
                     template as it (one tab is every tab). The edit is \
                     recorded against the instance's own site.",
                );
            }
            2 if confined => {
                let rect = line.child(live_id!(scope_all)).area().clipped_rect(cx);
                tip.show_with_options(
                    cx,
                    place(rect),
                    &format!(
                        "all {ty}s, held to the isolated branch: every {ty} \
                         inside it and none outside. Recorded against that \
                         branch, not the {ty} type."
                    ),
                );
            }
            2 => {
                let rect = line.child(live_id!(scope_all)).area().clipped_rect(cx);
                tip.show_with_options(
                    cx,
                    place(rect),
                    &format!(
                        "all {ty}s: every {ty} in the app. The edit is \
                         recorded against the type's own definition, so it \
                         is the type that changes."
                    ),
                );
            }
            3 => {
                let rect = line.child(live_id!(scope_isolated)).area().clipped_rect(cx);
                tip.show_with_options(
                    cx,
                    place(rect),
                    if !isolating {
                        "isolated: holds an \"all\" edit to the isolated \
                         branch. Nothing is isolated at the moment, so it \
                         changes nothing until something is."
                    } else if confined {
                        "isolated is ON: an \"all\" edit stays inside the \
                         isolated branch, and is recorded against that \
                         branch rather than the type. Turn it off to reach \
                         the whole app."
                    } else {
                        "isolated is OFF: an \"all\" edit reaches every one \
                         in the app, including the part the isolation is \
                         covering."
                    },
                );
            }
            _ => tip.hide(cx),
        }
    }

    /// Live code: apply the editor's text as it stands; a compile error
    /// puts the last good text back (the app never shows a blank widget)
    /// and says why under the editor.
    fn live_apply(&mut self, cx: &mut Cx) {
        let Some(sidebar) = self.sidebar.as_ref() else { return };
        let editor = sidebar
            .child(live_id!(shader_col))
            .child(live_id!(src_scroll))
            .child(live_id!(shader_src));
        let text = editor.text();
        if text == self.live_last_applied || !text.contains("fn") {
            return;
        }
        let _ = makepad_platform::shader_error::take();
        self.live_last_applied = text.clone();
        if let Err(error) = apply_fn_edit(cx, self, &text) {
            self.live_revert(cx, &error);
            return;
        }
        match makepad_platform::shader_error::take() {
            Some(err) => self.live_revert(cx, &err),
            None => {
                self.live_last_good = text;
                session().lock().unwrap().vibe_status = "live \u{2713}".to_string();
                self.live_error_pending = true;
                self.next_frame = cx.new_next_frame();
                self.redraw_sidebar(cx);
            }
        }
    }

    fn live_revert(&mut self, cx: &mut Cx, err: &str) {
        let first = err.trim().trim_start_matches("splash error:").trim();
        let first = first.split("; ").next().unwrap_or(first);
        let first = match first.find(" (from:") {
            Some(at) => &first[..at],
            None => first,
        };
        let first: String = first.chars().take(140).collect();
        session().lock().unwrap().vibe_status = format!("shader error: {first}");
        log!("TWEAK live shader error: {err}");
        if !self.live_last_good.is_empty() && self.live_last_good != self.live_last_applied {
            let good = self.live_last_good.clone();
            let _ = apply_fn_edit(cx, self, &good);
            let _ = makepad_platform::shader_error::take();
        }
        self.redraw_sidebar(cx);
    }

    /// The apply chunk for a change to one component of a structured row:
    /// a vec re-emits the whole vector (its components live together), an
    /// inset/metrics writes just the touched dotted sub-key.
    fn struct_component_chunk(&mut self, index: usize, comp: usize, v: f64) -> Option<String> {
        let kind = self.rows.get(index)?.struct_kind;
        let prop = self.rows.get(index)?.prop.clone();
        match kind {
            StructKind::Vec2 | StructKind::Vec3 | StructKind::Vec4 => {
                if let Some(slot) = self.rows[index].comp_vals.get_mut(comp) {
                    *slot = v;
                }
                let (n, pfx) = match kind {
                    StructKind::Vec2 => (2, "vec2f"),
                    StructKind::Vec3 => (3, "vec3f"),
                    _ => (4, "vec4f"),
                };
                let vals: Vec<String> = self.rows[index]
                    .comp_vals
                    .iter()
                    .take(n)
                    .map(|x| fmt_f64(*x))
                    .collect();
                Some(format!("{prop}: {pfx}({})", vals.join(" ")))
            }
            StructKind::Inset => {
                let key = ["left", "top", "right", "bottom"].get(comp)?;
                Some(format!("{prop}.{key}: {}", fmt_f64(v)))
            }
            StructKind::Metrics => {
                let key = ["descender", "line_gap", "line_scale"].get(comp)?;
                Some(format!("{prop}.{key}: {}", fmt_f64(v)))
            }
            StructKind::NoEditor | StructKind::None => None,
        }
    }

    fn sidebar_apply(&mut self, cx: &mut Cx, sel: &TweakPick, chunk: &str) {
        let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
        if widget.is_empty() {
            return;
        }
        if let Err(error) = apply_splash_chunk(cx, &widget, &sel.path, chunk, "sidebar") {
            log!("TWEAK sidebar apply failed: {error}");
        }
        self.rows_uid = 0;
    }

    /// Reset one property to its session-original value: apply it back
    /// through the same machinery and REMOVE its diff entries — a reset
    /// property is untouched again and /tweak/final never mentions it.
    fn reset_row(&mut self, cx: &mut Cx, sel: &TweakPick, row_index: usize) {
        let Some(row) = self.rows.get(row_index) else {
            return;
        };
        let prop = row.prop.clone();
        self.reset_prop(cx, sel, &prop);
    }

    /// Reset one prop to its session baseline (the FIRST diff entry's old
    /// value) and drop every ledger entry for it — a reset value is as if
    /// it was never touched: /tweak/state, /tweak/final and the changed
    /// indicators all forget it.
    fn reset_prop(&mut self, cx: &mut Cx, sel: &TweakPick, prop: &str) {
        let original = {
            let session = session().lock().unwrap();
            session
                .diff
                .iter()
                .find(|entry| entry.path == sel.path && entry.prop == prop)
                .map(|entry| entry.old.clone())
        };
        let Some(original) = original else {
            return; // untouched this session: nothing to reset
        };
        let prop = prop.to_string();
        let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
        if widget.is_empty() {
            return;
        }
        // What the printer wrote for nothing -- `null` for a None, `@-` for
        // the empty id -- the script spells `nil`.
        let original = match original.as_str() {
            "null" | "@-" => "nil".to_string(),
            _ => original,
        };
        let chunk = format!("{prop}: {original}");
        match eval_chunk(cx, &widget, &chunk) {
            Ok(()) => {
                let mut session = session().lock().unwrap();
                let removed: Vec<TweakDiffEntry> = session
                    .diff
                    .iter()
                    .filter(|entry| entry.path == sel.path && entry.prop == prop)
                    .cloned()
                    .collect();
                if !removed.is_empty() {
                    session.redo.clear();
                    session.undo.push(UndoStep::Reset {
                        path: sel.path.clone(),
                        prop: prop.clone(),
                        removed,
                    });
                    session.undo_open = false;
                }
                session
                    .diff
                    .retain(|entry| !(entry.path == sel.path && entry.prop == prop));
                session.suppress_until = cx.seconds_since_app_start() + SUPPRESS_LINGER;
                session.apply_gen += 1;
                drop(session);
                log!("TWEAK reset {} {} -> {}", sel.path, prop, original);
                self.rows_uid = 0;
                widget.redraw(cx);
                cx.redraw_all();
            }
            Err(error) => {
                log!("TWEAK reset failed for {} {}: {}", sel.path, prop, error);
            }
        }
    }

    /// Cmd+Z: pop the top edit gesture — restore the pre-gesture value and
    /// remove the gesture's ledger entries, as if it never happened.
    fn undo(&mut self, cx: &mut Cx) {
        let Some(step) = session().lock().unwrap().undo.pop() else {
            return;
        };
        match &step {
            UndoStep::Value {
                path,
                prop,
                old,
                seq_start,
                ..
            } => {
                let undone = if path.as_str() == "theme" {
                    self.theme_set(cx, prop.as_str(), old.as_str(), "undo")
                } else {
                    let Ok(widget) = resolve_widget_for_history(cx, path) else {
                        session().lock().unwrap().undo.push(step);
                        return;
                    };
                    if let Some(name) = prop.strip_prefix("const:") {
                        const_set(cx, &widget, path, name, old.parse::<f64>().ok(), "undo").map(|_| ())
                    } else {
                        eval_chunk(cx, &widget, &format!("{prop}: {old}"))
                    }
                };
                if let Err(error) = undone {
                    log!("TWEAK undo failed: {error}");
                    return;
                }
                let mut s = session().lock().unwrap();
                s.diff.retain(|e| {
                    !(e.path == *path && e.prop == *prop && e.seq >= *seq_start)
                });
                s.apply_gen += 1;
                s.undo_open = false;
                drop(s);
                log!("TWEAK undo {} {} -> {}", path, prop, old);
            }
            UndoStep::Reset {
                path,
                prop,
                removed,
            } => {
                let Ok(widget) = resolve_widget_for_history(cx, path) else {
                    session().lock().unwrap().undo.push(step);
                    return;
                };
                let last = removed.last().map(|e| e.new.clone()).unwrap_or_default();
                let chunk = format!("{prop}: {last}");
                if let Err(error) = eval_chunk(cx, &widget, &chunk) {
                    log!("TWEAK undo(reset) failed: {error}");
                    return;
                }
                let mut s = session().lock().unwrap();
                for entry in removed {
                    s.diff.push(entry.clone());
                }
                s.apply_gen += 1;
                s.undo_open = false;
                drop(s);
                log!("TWEAK undo reset {} {} -> {}", path, prop, last);
            }
        }
        session().lock().unwrap().redo.push(step);
        self.rows_uid = 0;
        cx.redraw_all();
    }

    /// Cmd+Shift+Z: replay the most recently undone gesture.
    fn redo(&mut self, cx: &mut Cx) {
        let Some(step) = session().lock().unwrap().redo.pop() else {
            return;
        };
        match &step {
            UndoStep::Value {
                path, prop, new, ..
            } => {
                let redone = if path.as_str() == "theme" {
                    self.theme_set(cx, prop.as_str(), new.as_str(), "redo")
                } else {
                    let Ok(widget) = resolve_widget_for_history(cx, path) else {
                        session().lock().unwrap().redo.push(step);
                        return;
                    };
                    if let Some(name) = prop.strip_prefix("const:") {
                        const_set(cx, &widget, path, name, new.parse::<f64>().ok(), "redo").map(|_| ())
                    } else {
                        let chunk = format!("{prop}: {new}");
                        apply_splash_chunk(cx, &widget, path, &chunk, "redo").map(|_| ())
                    }
                };
                if let Err(error) = redone {
                    log!("TWEAK redo failed: {error}");
                    return;
                }
                let mut s = session().lock().unwrap();
                let seq_start = s
                    .diff
                    .last()
                    .map(|e| e.seq)
                    .unwrap_or(0);
                s.undo.push(UndoStep::Value {
                    path: path.clone(),
                    prop: prop.clone(),
                    old: match &step {
                        UndoStep::Value { old, .. } => old.clone(),
                        _ => unreachable!(),
                    },
                    new: new.clone(),
                    seq_start,
                });
                s.undo_open = false;
                drop(s);
                log!("TWEAK redo {} {} -> {}", path, prop, new);
            }
            UndoStep::Reset {
                path,
                prop,
                removed,
            } => {
                let Ok(widget) = resolve_widget_for_history(cx, path) else {
                    session().lock().unwrap().redo.push(step);
                    return;
                };
                let baseline = removed.first().map(|e| e.old.clone()).unwrap_or_default();
                let chunk = format!("{prop}: {baseline}");
                if let Err(error) = eval_chunk(cx, &widget, &chunk) {
                    log!("TWEAK redo(reset) failed: {error}");
                    return;
                }
                let mut s = session().lock().unwrap();
                let (path_c, prop_c) = (path.clone(), prop.clone());
                s.diff
                    .retain(|e| !(e.path == path_c && e.prop == prop_c));
                s.undo.push(step.clone());
                s.apply_gen += 1;
                s.undo_open = false;
                drop(s);
                log!("TWEAK redo reset {} {}", path_c, prop_c);
            }
        }
        self.rows_uid = 0;
        cx.redraw_all();
    }

    /// Sidebar edits: every field action becomes one splash chunk applied
    /// to the selected instance — textbox, swatch, picker and the AI's
    /// /tweak/apply all land in the same diff entry per property.
    fn handle_sidebar_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let sel = session().lock().unwrap().pinned.clone();
        // The search box filters even with nothing selected.
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            // The extrusion readout, likewise: it acts on the VIEW, not on a
            // widget, and the exploded view is usually entered with nothing
            // selected at all. Handled in the second loop it went nowhere —
            // that one returns early when there is no selection, so the
            // number moved under the finger and the stack never opened.
            if self.spread_uid != 0 && widget_action.widget_uid.0 == self.spread_uid {
                match widget_action.cast::<FabValueInputAction>() {
                    FabValueInputAction::Changed(v) | FabValueInputAction::Ended(v) => {
                        cx.sploded_set_spread(v as f32);
                    }
                    FabValueInputAction::Reset => {
                        cx.sploded_set_spread(SPLODED_SPREAD_DEFAULT);
                        if let Some(ui) = self.spread_ui.as_ref() {
                            let field = ui.child(live_id!(value));
                            if let Some(mut field) = field.borrow_mut::<FabValueInput>() {
                                field.set_value(cx, SPLODED_SPREAD_DEFAULT as f64);
                            };
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if (self.note_text_uid != 0 && widget_action.widget_uid.0 == self.note_text_uid)
                || (self.prompt_field_uid != 0
                    && widget_action.widget_uid.0 == self.prompt_field_uid)
            {
                if let TextInputAction::Changed(text) = widget_action.cast::<TextInputAction>() {
                    let is_notes = widget_action.widget_uid.0 == self.note_text_uid;
                    if !is_notes {
                        // The prompt box is synced on every keystroke, not
                        // only on send: the draw re-seeds the box from the
                        // session whenever the caret is elsewhere, and a
                        // session that only knew about sent messages handed
                        // back an empty box -- or the last recalled one --
                        // the moment you clicked the widget you were writing
                        // about, with the caret left mid-string.
                        session().lock().unwrap().prompt = text.clone();
                    }
                    // A fresh `@` arms a widget pick: the next click in the
                    // app names something INTO the note. Counted rather than
                    // matched at the end, so an `@` typed mid-sentence arms
                    // it too, and deleting one disarms. Each box keeps its
                    // own count.
                    let ats = text.matches('@').count();
                    let before = if is_notes { self.note_at_count } else { self.prompt_at_count };
                    if ats > before {
                        let mut s = session().lock().unwrap();
                        s.mention = true;
                        s.mention_from_prompt = !is_notes;
                        drop(s);
                        log!("TWEAK @mention armed: click the widget to name it");
                        self.redraw_overlay(cx);
                    } else if ats < before {
                        session().lock().unwrap().mention = false;
                        self.redraw_overlay(cx);
                    }
                    if is_notes {
                        self.note_at_count = ats;
                    } else {
                        self.prompt_at_count = ats;
                    }
                    // The notes field is the only one whose text belongs to a
                    // record; the prompt's box belongs to nobody until it is
                    // sent. Guard on the field being SEEDED for the path,
                    // because the selection and the field can differ for a
                    // frame.
                    let showing = self.note_path(cx).as_deref()
                        == Some(self.note_key_shown.as_str());
                    let path = self.note_path(cx).filter(|_| showing && is_notes);
                    if let Some(path) = path {
                        let mut s = session().lock().unwrap();
                        let mut pinned = false;
                        if let Some(note) = s.notes.iter_mut().find(|n| n.path == path) {
                            note.text = text.clone();
                            pinned = !note.text.trim().is_empty();
                        }
                        if pinned {
                            let notes = s.notes.clone();
                            drop(s);
                            note_store_save(&notes);
                        }
                    }
                }
            }
            // The identity row's name field: a rename is a REQUEST, not a
            // live edit. The name is a LiveId the source assigned and every
            // `ids!(…)` lookup depends on, so renaming it under the running
            // app would break the app and leave the source lying. The AI does
            // the rename properly; this records what was asked for.
            if self.identity_uid != 0 && widget_action.widget_uid.0 == self.identity_uid {
                if let TextInputAction::Returned(text, _) = widget_action.cast::<TextInputAction>()
                {
                    self.request_rename(cx, text.trim());
                }
            }
            if self.tab_uids.contains(&widget_action.widget_uid.0)
                && widget_action.widget_uid.0 != 0
            {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    let index = self
                        .tab_uids
                        .iter()
                        .position(|uid| *uid == widget_action.widget_uid.0)
                        .unwrap_or(0);
                    self.panel_tab = match index {
                        1 => PanelTab::Shader,
                        2 => PanelTab::Tree,
                        3 => PanelTab::Theme,
                        4 => PanelTab::Spec,
                        _ => PanelTab::Props,
                    };
                    self.redraw_sidebar(cx);
                }
            }
            if self.tree_list_uid != 0 && widget_action.widget_uid.0 == self.tree_list_uid {
                match widget_action.cast::<FileTreeAction>() {
                    FileTreeAction::FileClicked(id) | FileTreeAction::FolderClicked(id) => {
                        // Tree node click: pin that widget, exactly like a
                        // body pick (drives 2D outline AND the 3D view).
                        let target = id.0;
                        // ...and put it ON SCREEN first. Selecting something
                        // behind an unselected tab or a closed fold outlines
                        // nothing and fills the panel with a widget nobody
                        // can see.
                        reveal_widget(cx, target);
                        let widget = cx.widget_tree().widget(WidgetUid(target));
                        if !widget.is_empty() {
                            let rect = widget.area().clipped_rect_union(cx);
                            let ids = cx.widget_tree().path_to(WidgetUid(target));
                            let path = ids
                                .iter()
                                .map(|id| live_id_token(*id))
                                .collect::<Vec<_>>()
                                .join(".");
                            let ty = widget
                                .widget_type_id()
                                .and_then(|type_id| {
                                    widget_type_names(cx).get(&type_id).copied()
                                })
                                .map(live_id_token)
                                .unwrap_or_else(|| "-".to_string());
                            session().lock().unwrap().pinned = Some(TweakPick {
                                uid: target,
                                path,
                                ty,
                                rect,
                                window_id: self.my_window.unwrap_or(0),
                                band: None,
                                level: 0,
                            });
                            self.rows_uid = 0;
                            self.redraw_overlay(cx);
                            self.redraw_sidebar(cx);
                        }
                    }
                    FileTreeAction::NodeHovered(id) => {
                        // Tree hover: outline that widget in the body/3D.
                        let widget = cx.widget_tree().widget(WidgetUid(id.0));
                        if !widget.is_empty() {
                            let rect = widget.area().clipped_rect_union(cx);
                            if rect.size.x > 0.0 {
                                session().lock().unwrap().hover = Some(TweakPick {
                                    uid: id.0,
                                    path: String::new(),
                                    ty: String::new(),
                                    rect,
                                    window_id: self.my_window.unwrap_or(0),
                                    band: None,
                                    level: 0,
                                });
                                self.tree_hover_active = true;
                                self.redraw_overlay(cx);
                            }
                        }
                    }
                    FileTreeAction::NodeHoverEnded(_) => {
                        if self.tree_hover_active {
                            self.tree_hover_active = false;
                            session().lock().unwrap().hover = None;
                            self.redraw_overlay(cx);
                        }
                    }
                    _ => {}
                }
            }
            if self.select_uid != 0 && widget_action.widget_uid.0 == self.select_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    let locked = {
                        let mut s = session().lock().unwrap();
                        s.selection_locked = !s.selection_locked;
                        // Nothing is under the pointer as far as the overlay
                        // is concerned any more; a stale hover outline would
                        // sit there until something else redrew it away.
                        s.hover = None;
                        s.selection_locked
                    };
                    log!(
                        "TWEAK select {}",
                        if locked { "off - selection locked, mouse to the app" } else { "on" }
                    );
                    self.redraw_sidebar(cx);
                    self.redraw_overlay(cx);
                }
            }
            if self.sploded_uid != 0 && widget_action.widget_uid.0 == self.sploded_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    // The 2.5D exploded z-layer view. Inspection-only while
                    // up (input belongs to the mode): exit with Escape or this button.
                    cx.sploded_toggle();
                    // The toggle is deferred to the next event; read the
                    // state it WILL have, not the one it still has.
                    self.sploded_armed = cx.sploded_will_be_active();
                    log!("TWEAK sploded view {}", if self.sploded_armed { "ON" } else { "off" });
                }
            }
            if self.search_uid != 0 && widget_action.widget_uid.0 == self.search_uid {
                match widget_action.cast::<TextInputAction>() {
                    TextInputAction::Changed(text) => {
                        self.filter = text.to_lowercase();
                        self.redraw_sidebar(cx);
                    }
                    TextInputAction::Escaped => {
                        self.filter.clear();
                        if let Some(sidebar) = self.sidebar.as_ref() {
                            sidebar
                                .child(live_id!(filter_row))
                                .child(live_id!(search))
                                .child(live_id!(input))
                                .set_text(cx, "");
                        }
                        cx.set_key_focus(Area::Empty);
                        self.redraw_sidebar(cx);
                    }
                    _ => {}
                }
            }
        }
        // The Theme tab edits the theme itself: no widget is selected.
        let theme = self.panel_tab == PanelTab::Theme;
        let Some(sel) = sel.or_else(|| theme.then(TweakPick::default)) else {
            return;
        };
        #[derive(Clone)]
        enum Edit {
            Apply(String),
            Revert(usize, String),
            HoldOn(usize),
            HoldOff,
            Eyedrop(String),
            /// Fill a just-opened colour popover's palette strip.
            Palette(u64),
            /// Pulse a theme colour by name (None: stop).
            PulseName(Option<String>),
        }
        let mut edits: Vec<Edit> = Vec::new();
        let mut resets: Vec<String> = Vec::new();
        let mut const_edits: Vec<(usize, Option<f64>)> = Vec::new();
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            let action_uid = widget_action.widget_uid.0;
            // The exploded view's spread knob: level separation, live.
            if self.shader_src_uid != 0 && action_uid == self.shader_src_uid {
                // The editor only reports document changes: live code
                // settles 200ms after the last keystroke.
                self.live_timer = cx.start_timeout(0.2);
                continue;
            }
            if action_uid != 0 && action_uid == self.scope_isolated_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    let confined = {
                        let mut s = session().lock().unwrap();
                        s.scope_unconfined = !s.scope_unconfined;
                        !s.scope_unconfined
                    };
                    log!(
                        "TWEAK scope isolated {}",
                        if confined { "on: all stays inside the isolated branch" } else { "off: all reaches the whole app" }
                    );
                    self.redraw_sidebar(cx);
                }
            }
            if action_uid != 0 && (action_uid == self.scope_this_uid || action_uid == self.scope_all_uid) {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    let all = action_uid == self.scope_all_uid;
                    session().lock().unwrap().scope_all = all;
                    log!("TWEAK scope {}", if all { "all — every widget of the type" } else { "this — specialise this instance" });
                    self.rows_uid = 0;
                    self.redraw_sidebar(cx);
                }
                continue;
            }
            if let Some((index, name)) = self.rows.iter().enumerate().find_map(|(i, b)| {
                if b.theme_uid != 0 && b.theme_uid == action_uid {
                    b.theme_match.clone().map(|n| (i, n))
                } else {
                    None
                }
            }) {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    edits.push(Edit::Apply(format!("{}: theme.{}", self.rows[index].prop, name)));
                }
                continue;
            }
            if self.states_pause_uid != 0 && action_uid == self.states_pause_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    self.states_paused = !self.states_paused;
                    if !self.states_paused {
                        self.states_frame = cx.new_next_frame();
                    }
                    self.redraw_sidebar(cx);
                }
                continue;
            }
            if self.view_center_uid != 0 && action_uid == self.view_center_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    self.view_center = !self.view_center;
                    self.apply_view_focus(cx);
                    self.redraw_sidebar(cx);
                }
                continue;
            }
            if self.view_zoom_uid != 0 && action_uid == self.view_zoom_uid {
                if let FabValueInputAction::Changed(v) =
                    widget_action.cast::<FabValueInputAction>()
                {
                    self.view_zoom = (v as f32).clamp(1.0, 4.0);
                    self.apply_view_focus(cx);
                }
                continue;
            }
            if self.tree_isolate_uid != 0 && action_uid == self.tree_isolate_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    if self.tree_isolate {
                        self.tree_isolate = false;
                        self.isolate_uid = 0;
                        log!("TWEAK tree isolate off");
                    } else if self.rows_uid != 0 && self.rows_uid != THEME_ROWS {
                        // Lock onto what is selected NOW, and stay there.
                        self.tree_isolate = true;
                        self.isolate_uid = self.rows_uid;
                        log!("TWEAK tree isolate on \u{2192} uid {}", self.isolate_uid);
                    } else {
                        session().lock().unwrap().vibe_status =
                            "select something to isolate".to_string();
                    }
                    // The tree is rebuilt from a different root, so the
                    // reveal has to run again for the new shape.
                    self.tree_scrolled_uid = 0;
                    self.apply_view_focus(cx);
                    self.redraw_sidebar(cx);
                    self.redraw_overlay(cx);
                }
                continue;
            }
            // The prompt strip's buttons and the Spec tab's cross. These sat
            // inside the note card's action block and went out with it; the
            // keys kept working, which is why nothing looked wrong.
            if self.spec_clear_uid != 0 && action_uid == self.spec_clear_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    self.spec_clear_notes(cx);
                }
                continue;
            }
            if self.prompt_queue_uid != 0 && action_uid == self.prompt_queue_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    self.prompt_queue(cx);
                }
                continue;
            }
            if self.prompt_send_uid != 0 && action_uid == self.prompt_send_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    self.prompt_send(cx);
                }
                continue;
            }
            if self.shader_fold_uid != 0 && action_uid == self.shader_fold_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    self.shader_src_open = !self.shader_src_open;
                    self.redraw_sidebar(cx);
                }
                continue;
            }
            // Composite-row fields (size pair, box legs, spacing, align
            // dots, link toggles) are not rows; resolve them first.
            if let Some((_, prop)) = self
                .composite_fields
                .iter()
                .find(|(uid, _)| *uid == action_uid)
                .cloned()
            {
                match widget_action.cast::<FabValueInputAction>() {
                    FabValueInputAction::Reset => {
                        resets.push(prop.clone());
                        continue;
                    }
                    FabValueInputAction::Changed(v) => {
                        // A Fill's weight or shrink: the whole Fill goes
                        // back, with the one field changed.
                        if let Some((axis, key)) = prop.split_once('#') {
                            let chunk = if axis == "abs_pos" {
                                Some(self.abs_chunk(key, v))
                            } else if axis == "cell" {
                                self.cell_chunk(key, &fmt_f64(v))
                            } else {
                                Some(self.fill_chunk(axis, key, &fmt_f64(v)))
                            };
                            if let Some(chunk) = chunk {
                                edits.push(Edit::Apply(chunk));
                            }
                            continue;
                        }
                        // Linked box editor: one leg drives all four.
                        let link_index = if prop.starts_with("margin.") {
                            Some(0)
                        } else if prop.starts_with("padding.") {
                            Some(1)
                        } else {
                            None
                        };
                        match link_index {
                            Some(li) if self.box_link[li] => {
                                let base = prop.split('.').next().unwrap_or("");
                                edits.push(Edit::Apply(format!(
                                    "{base}: Inset{{left: {v} top: {v} right: {v} bottom: {v}}}",
                                    v = fmt_f64(v)
                                )));
                            }
                            _ => edits.push(Edit::Apply(format!("{prop}: {}", fmt_f64(v)))),
                        }
                        continue;
                    }
                    FabValueInputAction::Ended(_) => {
                        edits.push(Edit::HoldOff);
                        continue;
                    }
                    _ => {}
                }
                match widget_action.cast::<TextInputAction>() {
                    TextInputAction::Changed(text) => {
                        let text = text.trim().to_string();
                        // A size field takes a mode, points, a CSS spelling
                        // or an expression; a bound or the aspect takes the
                        // same and, emptied, clears; the chunk is the one
                        // the engine accepts for each.
                        let chunk = if prop == "width" || prop == "height" {
                            size_chunk(&prop, &text)
                        } else if let Some((axis, key)) = prop.split_once('#') {
                            if axis == "cell" {
                                self.cell_chunk(key, &text)
                            } else {
                                (!text.is_empty()).then(|| self.fill_chunk(axis, key, &text))
                            }
                        } else if prop == "columns" || prop == "rows" {
                            tracks_chunk(&prop, &text)
                        } else if prop == "areas" {
                            areas_chunk(&text)
                        } else if prop.starts_with("min_") || prop.starts_with("max_") {
                            bound_chunk(&prop, &text)
                        } else if prop == "aspect" {
                            aspect_chunk(&text)
                        } else if prop == "container_id" {
                            // A name is an id; emptied, the source's own
                            // value comes back, since an id cannot be nil.
                            if text.is_empty() {
                                resets.push(prop.clone());
                                None
                            } else {
                                container_chunk(&text)
                            }
                        } else {
                            (!text.is_empty()).then(|| format!("{prop}: {text}"))
                        };
                        if let Some(chunk) = chunk {
                            edits.push(Edit::Apply(chunk));
                        }
                        continue;
                    }
                    _ => {}
                }
                continue;
            }
            if let Some((_, chunk)) = self
                .composite_clicks
                .iter()
                .find(|(uid, _)| *uid == action_uid)
                .cloned()
            {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    edits.push(Edit::Apply(chunk));
                }
                continue;
            }
            if self.abs_uid != 0 && action_uid == self.abs_uid {
                if let CheckBoxAction::Change(on) = widget_action.cast::<CheckBoxAction>() {
                    edits.push(Edit::Apply(
                        if on { "abs_pos: vec2(0, 0)" } else { "abs_pos: nil" }.to_string(),
                    ));
                }
                continue;
            }
            if self.convert_uid != 0 && action_uid == self.convert_uid {
                if let ButtonAction::Clicked(_) = widget_action.cast::<ButtonAction>() {
                    let ty = session().lock().unwrap().pinned.as_ref().map(|s| s.ty.clone());
                    let to = if ty.as_deref() == Some("Grid") { "flex" } else { "grid" };
                    self.request_convert(cx, to);
                }
                continue;
            }
            if let Some(link_index) = self.box_link_uids.iter().position(|uid| *uid == action_uid)
            {
                if let CheckBoxAction::Change(v) = widget_action.cast::<CheckBoxAction>() {
                    self.box_link[link_index] = v;
                }
                continue;
            }
            // A structured value's component field (vec x/y, inset leg,
            // metric): apply the whole value (vec) or the touched dotted
            // sub-key (inset/metrics), which the grammar accepts.
            if let Some((index, comp)) = self.rows.iter().enumerate().find_map(|(i, b)| {
                b.comp_uids
                    .iter()
                    .position(|u| *u == action_uid)
                    .map(|c| (i, c % comp_count(b.struct_kind)))
            }) {
                self.doc_row = Some(index);
                match widget_action.cast::<FabValueInputAction>() {
                    FabValueInputAction::Changed(v) => {
                        if let Some(chunk) = self.struct_component_chunk(index, comp, v) {
                            edits.push(Edit::Apply(chunk));
                        }
                    }
                    FabValueInputAction::Ended(_) => edits.push(Edit::HoldOff),
                    _ => {}
                }
                continue;
            }
            let Some((index, binding)) = self
                .rows
                .iter()
                .enumerate()
                .find(|(_, b)| {
                    (b.field_uid != 0 && b.field_uid == action_uid)
                        || (b.swatch_uid != 0 && b.swatch_uid == action_uid)
                        || b.alt_uids.iter().any(|(u, _)| *u == action_uid)
                })
                .map(|(i, b)| (i, b.clone()))
            else {
                continue;
            };
            self.doc_row = Some(index);
            let is_swatch = (binding.swatch_uid != 0 && binding.swatch_uid == action_uid)
                || binding.alt_uids.iter().any(|(u, s)| *u == action_uid && *s);
            match binding.kind {
                // CASCADE rows are read-only labels; nothing to apply.
                RowKind::Info => {}
                RowKind::Num if binding.const_ref.is_some() => {
                    match widget_action.cast::<FabValueInputAction>() {
                        FabValueInputAction::Changed(v) => const_edits.push((index, Some(v))),
                        FabValueInputAction::Ended(v) => {
                            // A typed value arrives only as Ended.
                            if binding.value.parse::<f64>().ok() != Some(v) {
                                const_edits.push((index, Some(v)));
                            }
                            edits.push(Edit::HoldOff);
                        }
                        FabValueInputAction::Reset => const_edits.push((index, None)),
                        _ => {}
                    }
                }
                RowKind::Num => match widget_action.cast::<FabValueInputAction>() {
                    FabValueInputAction::Changed(v) => {
                        edits.push(Edit::Apply(format!("{}: {}", binding.prop, fmt_f64(v))));
                    }
                    FabValueInputAction::Ended(_) => {
                        edits.push(Edit::HoldOff);
                    }
                    FabValueInputAction::Reset => {
                        resets.push(binding.prop.clone());
                    }
                    _ => {}
                },
                RowKind::Bool => match widget_action.cast::<CheckBoxAction>() {
                    CheckBoxAction::Change(v) => {
                        edits.push(Edit::Apply(format!("{}: {}", binding.prop, v)));
                    }
                    _ => {}
                },
                RowKind::Text => match widget_action.cast::<TextInputAction>() {
                    // Text applies LIVE as you type; Enter/blur only end the
                    // interaction, Escape restores the focus-start value.
                    TextInputAction::KeyFocus => {
                        edits.push(Edit::HoldOn(index));
                    }
                    TextInputAction::Changed(text) => {
                        let value = if binding.quoted {
                            format!("{text:?}")
                        } else {
                            text.trim().to_string()
                        };
                        if !value.is_empty() {
                            edits.push(Edit::Apply(format!("{}: {}", binding.prop, value)));
                        }
                    }
                    TextInputAction::Returned(..) | TextInputAction::KeyFocusLost => {
                        edits.push(Edit::HoldOff);
                    }
                    TextInputAction::Escaped => {
                        if let Some((row, origin)) = self.text_edit_origin.clone() {
                            if row == index {
                                edits.push(Edit::Revert(index, origin));
                            }
                        }
                        edits.push(Edit::HoldOff);
                    }
                    _ => {}
                },
                RowKind::Color if !is_swatch => match widget_action.cast::<TextInputAction>() {
                    // The hex box: live-apply only when the text parses;
                    // Enter/blur accept, invalid input reverts.
                    TextInputAction::KeyFocus => {
                        edits.push(Edit::HoldOn(index));
                    }
                    TextInputAction::Changed(text) => {
                        if let Some((rgba, had_alpha)) = parse_hex(&text) {
                            let hex = format_hex(rgba, had_alpha || true);
                            edits.push(Edit::Apply(format!("{}: {}", binding.prop, hex)));
                        }
                    }
                    TextInputAction::Returned(text, _) => {
                        match parse_hex(&text) {
                            Some((rgba, _)) => {
                                let hex = format_hex(rgba, true);
                                edits.push(Edit::Apply(format!("{}: {}", binding.prop, hex)));
                            }
                            None => {
                                // Invalid: revert the field to the current value.
                                edits.push(Edit::Revert(index, binding.value.clone()));
                            }
                        }
                        edits.push(Edit::HoldOff);
                    }
                    TextInputAction::KeyFocusLost => {
                        edits.push(Edit::HoldOff);
                    }
                    TextInputAction::Escaped => {
                        edits.push(Edit::Revert(index, binding.value.clone()));
                        edits.push(Edit::HoldOff);
                    }
                    _ => {}
                },
                RowKind::Color => match widget_action.cast::<FabColorPickAction>() {
                    FabColorPickAction::Changed(v) => {
                        let hex = format_hex([v.x, v.y, v.z, v.w], true);
                        edits.push(Edit::Apply(format!("{}: {}", binding.prop, hex)));
                    }
                    FabColorPickAction::Opened => {
                        edits.push(Edit::HoldOn(index));
                        edits.push(Edit::Palette(binding.swatch_uid));
                    }
                    FabColorPickAction::Closed => {
                        edits.push(Edit::HoldOff);
                    }
                    FabColorPickAction::Eyedropper => {
                        edits.push(Edit::HoldOff);
                        edits.push(Edit::Eyedrop(binding.prop.clone()));
                    }
                    FabColorPickAction::PaletteHover(name) => {
                        edits.push(Edit::PulseName(name));
                    }
                    FabColorPickAction::PalettePick(name) => {
                        // Bind by reference: the ledger says `theme.color_x`.
                        edits.push(Edit::HoldOff);
                        edits.push(Edit::PulseName(None));
                        edits.push(Edit::Apply(format!("{}: theme.{name}", binding.prop)));
                    }
                    _ => {}
                },
            }
        }
        for prop in resets {
            if theme {
                self.theme_reset(cx, &prop);
            } else {
                self.reset_prop(cx, &sel, &prop);
            }
        }
        for (index, value) in const_edits {
            let Some(cref) = self.rows.get(index).and_then(|r| r.const_ref.clone()) else { continue };
            let widget = cx.widget_tree().widget(WidgetUid(sel.uid));
            match const_set(cx, &widget, &sel.path, &cref.name, value, "sidebar") {
                Ok((_, new)) => {
                    if let Some(row) = self.rows.get_mut(index) {
                        row.value = fmt_f64(new as f64);
                        row.changed = new != cref.initial;
                    }
                    self.redraw_sidebar(cx);
                }
                Err(error) => log!("TWEAK shader constant apply failed: {error}"),
            }
        }
        for edit in edits {
            match edit {
                Edit::Apply(chunk) if theme => self.theme_apply_chunk(cx, &chunk),
                Edit::Apply(chunk) => self.sidebar_apply(cx, &sel, &chunk),
                Edit::Eyedrop(prop) => {
                    log!("TWEAK eyedropper armed for {prop} — click a pixel in the app");
                    session().lock().unwrap().eyedrop = Some(prop);
                    cx.set_cursor(MouseCursor::Crosshair);
                }
                Edit::Palette(uid) => {
                    let entries = self.palette_entries();
                    let swatch = cx.widget_tree().widget(WidgetUid(uid));
                    if let Some(mut pick) = swatch.borrow_mut::<FabColorPick>() {
                        pick.set_palette(cx, entries);
                    };
                }
                Edit::PulseName(name) => {
                    let color = name.and_then(|n| {
                        self.theme_colors.iter().find(|(k, _, _)| *k == n).map(|(_, c, _)| *c)
                    });
                    self.pulse_pinned = color.is_some();
                    self.set_pulse(cx, color);
                }
                Edit::Revert(index, value) => {
                    // Push the origin value back through the same path, then
                    // refresh the field itself.
                    if let Some(row) = self.rows.get(index) {
                        let text = if row.quoted {
                            format!("{value:?}")
                        } else {
                            value.clone()
                        };
                        let chunk = format!("{}: {}", row.prop, text);
                        if theme {
                            self.theme_apply_chunk(cx, &chunk);
                        } else {
                            self.sidebar_apply(cx, &sel, &chunk);
                        }
                    }
                    if !theme {
                        self.rows_uid = 0;
                    }
                }
                Edit::HoldOn(index) => {
                    session().lock().unwrap().edit_hold = true;
                    let origin = self.rows.get(index).map(|r| r.value.clone());
                    if let Some(origin) = origin {
                        self.text_edit_origin = Some((index, origin));
                    }
                    self.redraw_overlay(cx);
                }
                Edit::HoldOff => {
                    let mut s = session().lock().unwrap();
                    s.edit_hold = false;
                    // Gesture boundary: the next apply starts a NEW undo
                    // step (two scrubs on one prop never merge).
                    s.undo_open = false;
                    drop(s);
                    self.text_edit_origin = None;
                    self.redraw_overlay(cx);
                }
            }
        }
    }

    /// Scroll the property list so the cascade level's header row is at
    /// the top (origin-dot click-through).
    fn scroll_to_cascade_level(&mut self, cx: &mut Cx, level: usize) {
        let entries = self.build_visible();
        let target = entries.iter().position(|entry| {
            matches!(entry, VisKind::Prop(index)
                if self.rows[*index].section == SectionKind::Cascade
                    && self.rows[*index].prop.starts_with(&format!("L{level} ")))
        });
        if let (Some(target), Some(sidebar)) = (target, self.sidebar.as_ref()) {
            let list_ref = sidebar.child(live_id!(props_wrap)).child(live_id!(props));
            {
                if let Some(mut list) = list_ref.borrow_mut::<PortalList>() {
                    list.set_first_id_and_scroll(target, 0.0);
                }
            }
            drop(list_ref);
        }
        let _ = cx;
    }

    fn redraw_sidebar(&mut self, cx: &mut Cx) {
        if let Some(sidebar) = &self.sidebar {
            sidebar.redraw(cx);
        }
        if let Some(sidebar_list) = &self.sidebar_list {
            sidebar_list.redraw(cx);
        }
        cx.redraw_all();
    }

    /// The four corner-handle centres for the pinned rect (TL, TR, BR, BL).
    fn radius_handle_centers(rect: Rect) -> [Vec2d; 4] {
        const HANDLE_INSET: f64 = 5.0;
        [
            dvec2(rect.pos.x + HANDLE_INSET, rect.pos.y + HANDLE_INSET),
            dvec2(
                rect.pos.x + rect.size.x - HANDLE_INSET,
                rect.pos.y + HANDLE_INSET,
            ),
            dvec2(
                rect.pos.x + rect.size.x - HANDLE_INSET,
                rect.pos.y + rect.size.y - HANDLE_INSET,
            ),
            dvec2(
                rect.pos.x + HANDLE_INSET,
                rect.pos.y + rect.size.y - HANDLE_INSET,
            ),
        ]
    }

    /// Diagonal-inward unit direction per corner: dragging inward increases
    /// the radius, outward decreases it.
    fn radius_inward(corner: usize) -> Vec2d {
        match corner {
            0 => dvec2(1.0, 1.0),
            1 => dvec2(-1.0, 1.0),
            2 => dvec2(-1.0, -1.0),
            _ => dvec2(1.0, -1.0),
        }
    }

    /// The cascade level palette: instance orange, then blue, purple,
    /// green, gray tail — the same colors the origin dots and the cascade
    /// rows share.
    fn level_color(level: usize) -> Vec4f {
        match level {
            0 => vec4(1.0, 0.62, 0.13, 1.0),
            1 => vec4(0.19, 0.78, 1.0, 1.0),
            2 => vec4(0.72, 0.5, 1.0, 1.0),
            3 => vec4(0.35, 0.85, 0.55, 1.0),
            _ => vec4(0.55, 0.55, 0.55, 1.0),
        }
    }

    /// Right edge of the app viewport: overlay drawing (outlines, tags,
    /// handles, strokes) never crosses into the panel band.
    fn overlay_max_x(&self, pass_size: Vec2d) -> f64 {
        if self.band.size.x > 0.0 {
            self.band.pos.x
        } else {
            pass_size.x
        }
    }

    /// Where a layout rect lands ON SCREEN once the view transform has had
    /// its say.
    ///
    /// The overlay draws on the WINDOW pass, which carries no camera; the app
    /// draws through the scene pass, which does. So the moment the view is
    /// centred or zoomed, a widget's layout rect and the pixels it covers are
    /// two different places, and an outline drawn at the layout rect sits
    /// where the widget used to be. Projecting by hand puts it back on its
    /// widget. Flat transforms are a scale about a point, so a rect stays a
    /// rect and two corners are enough. Identity when nothing is transforming.
    fn screen_rect(&self, cx: &Cx2d, rect: Rect) -> Rect {
        // The explode has its own route — marks handed to the pass owner,
        // drawn on the widget's own plane — so this is the flat transform's
        // business only.
        if rect.size.x <= 0.0 || !cx.sploded_transformed() || cx.sploded_active() {
            return rect;
        }
        let pass = cx.current_pass_size();
        let Some(tl) = cx.sploded_project(pass, rect.pos, 0.0) else {
            return rect;
        };
        let br = cx
            .sploded_project(pass, rect.pos + rect.size, 0.0)
            .unwrap_or(rect.pos + rect.size);
        Rect { pos: tl, size: br - tl }
    }

    /// Clip a rect to the app viewport; None when nothing remains visible.
    fn clip_to_viewport(&self, cx: &Cx2d, rect: Rect) -> Option<Rect> {
        let max_x = self.overlay_max_x(cx.current_pass_size());
        if rect.pos.x >= max_x {
            return None;
        }
        let mut out = rect;
        if out.pos.x + out.size.x > max_x {
            out.size.x = max_x - out.pos.x;
        }
        Some(out)
    }

    fn draw_pick(&mut self, cx: &mut Cx2d, pick: &TweakPick, style: PickStyle) {
        if pick.rect.size.x <= 0.0 || pick.rect.size.y <= 0.0 {
            return;
        }
        let dpi = cx.current_dpi_factor().max(1.0) as f32;
        self.draw_outline.dpi = dpi;
        match style {
            PickStyle::Pinned => {
                // The SELECTION never wears a box ON its edge: the whole
                // point of pinning a widget is seeing how it actually
                // renders, and an outline sits exactly on the edge pixels
                // being judged. Four viewfinder corners mark it instead.
                //
                // The corners alone were too quiet though — hovering drew a
                // full blue box and clicking replaced it with four small
                // ticks, which reads as "the click did nothing". So the
                // brackets now come with a dashed hairline held 4pt OFF the
                // widget: unmistakably still selected, and not one pixel of
                // the thing being judged is touched.
                let ring = selection_ring(pick.rect);
                self.draw_outline.border_color = vec4(0.19, 0.78, 1.0, 0.55);
                self.draw_outline.fill_color = vec4(0.0, 0.0, 0.0, 0.0);
                self.draw_outline.border_size = 1.0;
                self.draw_outline.dash = 1.0;
                if let Some(ring) = self.clip_to_viewport(cx, ring) {
                    self.draw_outline.draw_abs(cx, ring);
                }
                self.draw_corner_brackets(cx, pick.rect);
                return; // no fill, no label chip
            }
            PickStyle::Hover => {
                self.draw_outline.border_color = vec4(0.19, 0.78, 1.0, 1.0);
                self.draw_outline.fill_color = vec4(0.0, 0.0, 0.0, 0.0);
                self.draw_outline.border_size = 1.0;
                self.draw_outline.dash = 0.0;
            }
            PickStyle::Mention => {
                self.draw_outline.border_color = vec4(1.0, 0.78, 0.13, 1.0);
                self.draw_outline.fill_color = vec4(1.0, 0.78, 0.13, 0.07);
                self.draw_outline.border_size = 2.0;
                self.draw_outline.dash = 0.0;
            }
            PickStyle::PinnedQuiet => {
                // Tweaking in progress: a faint stippled HAIRLINE (one
                // device pixel), outset 1pt so the widget's own edge pixels
                // — the thing being judged — stay untouched.
                self.draw_outline.border_color = vec4(1.0, 0.62, 0.13, 0.25);
                self.draw_outline.fill_color = vec4(0.0, 0.0, 0.0, 0.0);
                self.draw_outline.border_size = 1.0 / dpi;
                self.draw_outline.dash = 1.0;
                let outset = Rect {
                    pos: dvec2(pick.rect.pos.x - 1.0, pick.rect.pos.y - 1.0),
                    size: dvec2(pick.rect.size.x + 2.0, pick.rect.size.y + 2.0),
                };
                if let Some(outset) = self.clip_to_viewport(cx, outset) {
                    self.draw_outline.draw_abs(cx, outset);
                }
                return; // no label chip, no fill
            }
        }
        let Some(outline_rect) = self.clip_to_viewport(cx, pick.rect) else {
            return; // fully under the panel: nothing to outline or label
        };
        self.draw_outline.draw_abs(cx, outline_rect);

        // No pick banner: the panel footer carries the identity (the
        // full-path label used to stretch across the app).
    }

    /// The selection mark: four `⌐`-style corner brackets, outset from the
    /// pinned rect so the widget's own rendering (and the margin gutter the
    /// person is about to drag) stays completely visible.
    fn draw_corner_brackets(&mut self, cx: &mut Cx2d, rect: Rect) {
        const OUTSET: f64 = 5.0;
        const ARM: f64 = 9.0;
        const THICK: f64 = 2.0;
        let max_x = self.overlay_max_x(cx.current_pass_size());
        self.draw_outline.border_color = vec4(0.0, 0.0, 0.0, 0.0);
        self.draw_outline.border_size = 0.0;
        self.draw_outline.dash = 0.0;
        self.draw_outline.fill_color = vec4(0.19, 0.78, 1.0, 1.0);
        let left = rect.pos.x - OUTSET;
        let top = rect.pos.y - OUTSET;
        let right = rect.pos.x + rect.size.x + OUTSET;
        let bottom = rect.pos.y + rect.size.y + OUTSET;
        // (horizontal arm, vertical arm) per corner, arms pointing inward.
        let arms = [
            (dvec2(left, top), dvec2(ARM, THICK), dvec2(left, top), dvec2(THICK, ARM)),
            (
                dvec2(right - ARM, top),
                dvec2(ARM, THICK),
                dvec2(right - THICK, top),
                dvec2(THICK, ARM),
            ),
            (
                dvec2(left, bottom - THICK),
                dvec2(ARM, THICK),
                dvec2(left, bottom - ARM),
                dvec2(THICK, ARM),
            ),
            (
                dvec2(right - ARM, bottom - THICK),
                dvec2(ARM, THICK),
                dvec2(right - THICK, bottom - ARM),
                dvec2(THICK, ARM),
            ),
        ];
        for (h_pos, h_size, v_pos, v_size) in arms {
            for (pos, size) in [(h_pos, h_size), (v_pos, v_size)] {
                if pos.x + size.x > max_x {
                    continue; // never into the panel band
                }
                self.draw_outline.draw_abs(cx, Rect { pos, size });
            }
        }
    }

    /// Corner handles for the radius-like input (the first of the
    /// direct-manipulation handle family): fab-styled dots, visible whenever
    /// the pinned widget exposes a radius — and during the drag itself.
    fn draw_radius_handles(&mut self, cx: &mut Cx2d, rect: Rect) {
        let max_x = self.overlay_max_x(cx.current_pass_size());
        for center in Self::radius_handle_centers(rect) {
            if center.x + 4.0 > max_x {
                continue; // never draw handles into the panel band
            }
            self.draw_stroke.stroke_color = vec4(0.04, 0.04, 0.04, 0.9);
            self.draw_stroke.draw_abs(
                cx,
                Rect {
                    pos: dvec2(center.x - 4.0, center.y - 4.0),
                    size: dvec2(8.0, 8.0),
                },
            );
            self.draw_stroke.stroke_color = vec4(0.337, 0.502, 0.761, 1.0);
            self.draw_stroke.draw_abs(
                cx,
                Rect {
                    pos: dvec2(center.x - 3.0, center.y - 3.0),
                    size: dvec2(6.0, 6.0),
                },
            );
        }
    }

    /// Is `abs` on the footer's path line? Read at event time, so it answers
    /// for the frame actually on screen.
    fn footer_path_hit(&self, cx: &Cx, abs: Vec2d) -> bool {
        let Some(sidebar) = self.sidebar.as_ref() else { return false };
        let rect = sidebar
            .child(live_id!(ident_footer))
            .child(live_id!(path_row))
            .area()
            .clipped_rect(cx);
        rect.size.y > 0.0 && rect.contains(abs)
    }

    /// Put the selection's full path on the clipboard. The label shows a
    /// head-clipped version because the panel is narrow; what gets copied is
    /// the whole id path — the same string `/tweak/apply` and `/snap` take,
    /// so it is a reference anything can act on, not just read.
    fn copy_footer_path(&mut self, cx: &mut Cx) {
        let Some(sel) = session().lock().unwrap().pinned.clone() else { return };
        let path = self.sel_ref(cx, sel.uid);
        cx.copy_to_clipboard(&path);
        log!("TWEAK copied the selection path: {path}");
        // Say so where the path was: a clipboard write is invisible
        // otherwise, and a click that shows nothing reads as a dead click.
        // `draw_sidebar` puts the path back when the beat is up.
        self.footer_copied_until = cx.seconds_since_app_start() + FOOTER_COPIED_LINGER;
        self.next_frame = cx.new_next_frame();
        self.redraw_sidebar(cx);
    }

    /// ISOLATE, in the app itself: everything outside the isolated widget is
    /// covered, so the one thing being worked on stands alone.
    ///
    /// The hole is a QUAD, not a rect, because in the exploded view a widget
    /// sits on a tilted plane and its rect projects to a parallelogram — the
    /// flat four-band cover that works in 2D would blank the very thing it is
    /// meant to reveal. Scanline strips take the general shape for both: one
    /// strip per couple of points, each clipped to where the quad actually is
    /// at that height.
    ///
    /// Nothing is hidden, moved or re-laid-out: the widget renders exactly
    /// where and how it normally does, which is the only way what you see is
    /// what you are judging. Input is untouched too — you can still click
    /// your way out.
    fn draw_isolate_scrim(&mut self, cx: &mut Cx2d, hole: [Vec2d; 4]) {
        /// Scanline height. Small enough that a tilted edge reads as a line
        /// rather than a staircase, large enough not to flood the draw list.
        const STRIP: f64 = 2.0;
        let size = cx.current_pass_size();
        let max_x = self.overlay_max_x(size);
        let top = hole.iter().map(|p| p.y).fold(f64::INFINITY, f64::min).max(0.0);
        let bottom = hole
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max)
            .min(size.y);
        self.draw_outline.dpi = cx.current_dpi_factor().max(1.0) as f32;
        self.draw_outline.dash = 0.0;
        self.draw_outline.border_size = 0.0;
        self.draw_outline.border_color = vec4(0.0, 0.0, 0.0, 0.0);
        // Opaque, not a dim: a scrim that lets a few percent through shows a
        // seam wherever it meets different content behind it, which reads as
        // a rendering bug rather than as "the rest is out of the way".
        self.draw_outline.fill_color = vec4(0.09, 0.09, 0.10, 1.0);
        let mut band = |this: &mut Self, cx: &mut Cx2d, x: f64, y: f64, w: f64, h: f64| {
            if w > 0.0 && h > 0.0 {
                this.draw_outline
                    .draw_abs(cx, Rect { pos: dvec2(x, y), size: dvec2(w, h) });
            }
        };
        band(self, cx, 0.0, 0.0, max_x, top);
        band(self, cx, 0.0, bottom, max_x, size.y - bottom);
        let mut y = top;
        while y < bottom {
            let h = STRIP.min(bottom - y);
            match Self::quad_span_at(&hole, y + h * 0.5) {
                Some((left, right)) => {
                    band(self, cx, 0.0, y, left.min(max_x), h);
                    band(self, cx, right.max(0.0), y, max_x - right, h);
                }
                None => band(self, cx, 0.0, y, max_x, h),
            }
            y += h;
        }
    }

    /// Where a convex quad spans horizontally at height `y` — the two points
    /// its edges cross that line. `None` when the line misses it entirely.
    fn quad_span_at(quad: &[Vec2d; 4], y: f64) -> Option<(f64, f64)> {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for i in 0..4 {
            let (a, b) = (quad[i], quad[(i + 1) % 4]);
            if (a.y - y) * (b.y - y) > 0.0 || (a.y - b.y).abs() < 1.0e-9 {
                continue;
            }
            let t = (y - a.y) / (b.y - a.y);
            let x = a.x + (b.x - a.x) * t;
            lo = lo.min(x);
            hi = hi.max(x);
        }
        (hi >= lo).then_some((lo, hi))
    }

    /// The pin badge: a small amber pin on every widget that carries a PINNED
    /// note, so a note written last week announces itself instead of waiting
    /// to be stumbled on. Not a button — a mark you can click.
    fn ensure_badge_ui(&mut self, cx: &mut Cx) {
        if self.badge_ui.is_some() {
            return;
        }
        // Amber describes, blue constrains, green does both. The colour is
        // baked into each instance, not applied per badge.
        let note = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                Icon {
                    width: Fit
                    height: Fit
                    icon_walk: Walk{width: 11 height: Fit}
                    draw_icon +: {
                        color: #xffc74a
                        svg: crate_resource("self:resources/icons/note_pin.svg")
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let rule = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                Icon {
                    width: Fit
                    height: Fit
                    icon_walk: Walk{width: 11 height: Fit}
                    draw_icon +: {
                        color: #x59b3ff
                        svg: crate_resource("self:resources/icons/note_pin.svg")
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        let both = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                Icon {
                    width: Fit
                    height: Fit
                    icon_walk: Walk{width: 11 height: Fit}
                    draw_icon +: {
                        color: #x57d98a
                        svg: crate_resource("self:resources/icons/note_pin.svg")
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        self.badge_ui = Some([note, rule, both]);
    }

    /// Re-resolve which widgets carry a pinned note. Walks the whole tree, so
    /// it runs on a timer, not per frame; the badges themselves ride the live
    /// rects of the uids it finds.
    fn refresh_badges(&mut self, cx: &mut Cx2d) {
        let now = cx.seconds_since_app_start();
        if now < self.badges_at + BADGE_REFRESH {
            return;
        }
        self.badges_at = now;
        // A badge means "something is written about this widget", which is
        // the only test left once pinning is gone: a record exists for every
        // widget a prompt was ever sent about, and those must not all wear a
        // mark. Which KIND it is comes back with it.
        let keys: Vec<(String, BadgeKind)> = {
            let mut s = session().lock().unwrap();
            s.load_notes();
            s.notes
                .iter()
                .filter_map(|n| {
                    let note = !n.text.trim().is_empty();
                    let rule = !n.rules.trim().is_empty();
                    let kind = match (note, rule) {
                        (true, true) => BadgeKind::Both,
                        (true, false) => BadgeKind::Note,
                        (false, true) => BadgeKind::Rule,
                        (false, false) => return None,
                    };
                    Some((n.path.clone(), kind))
                })
                .collect()
        };
        self.badge_targets.clear();
        if keys.is_empty() {
            return;
        }
        for (uid, path) in readable_paths(cx) {
            if let Some((_, kind)) = keys.iter().find(|(key, _)| *key == path) {
                self.badge_targets.push((uid, path, *kind));
            }
        }
    }

    /// Draw a pin on each badged widget and remember where it landed.
    fn draw_badges(&mut self, cx: &mut Cx2d, scope: &mut Scope, window_id: Option<usize>) {
        self.badge_rects.clear();
        if self.badge_targets.is_empty() {
            return;
        }
        self.ensure_badge_ui(cx);
        let pins = self.badge_ui.as_ref().unwrap().clone();
        let max_x = self.overlay_max_x(cx.current_pass_size());
        let targets = self.badge_targets.clone();
        for (uid, _, kind) in targets {
            let widget = cx.widget_tree().widget(WidgetUid(uid));
            if widget.is_empty() {
                continue;
            }
            let rect = live_rect(cx, &widget);
            if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
                continue; // not drawn this frame
            }
            let _ = window_id;
            // Top-right, just outside the selection ring, so it never sits on
            // the widget's own pixels.
            let badge = |r: Rect| {
                dvec2(
                    (r.pos.x + r.size.x - 4.0).min(max_x - BADGE_SIZE - 1.0),
                    (r.pos.y - BADGE_SIZE + 2.0).max(0.0),
                )
            };
            // Drawn where the widget IS on screen, remembered where it is in
            // LAYOUT: the badge is painted on the window pass but clicked
            // with a pointer the view transform has already un-projected.
            let pos = badge(self.screen_rect(cx, rect));
            if pos.x < 0.0 {
                continue;
            }
            let mut walk = Walk::fit();
            walk.abs_pos = Some(pos);
            let _ = pins[kind.index()].draw_walk(cx, scope, walk);
            self.badge_rects.push((
                Rect { pos: badge(rect), size: dvec2(BADGE_SIZE, BADGE_SIZE) },
                uid,
            ));
        }
    }

    fn draw_stroke_points(&mut self, cx: &mut Cx2d, points: &[(f64, f64)]) {
        let max_x = self.overlay_max_x(cx.current_pass_size());
        let points: Vec<(f64, f64)> = points
            .iter()
            .copied()
            .filter(|(x, _)| *x < max_x)
            .collect();
        let points = &points[..];
        // A freehand stroke as a dense dot chain: no polyline shader needed,
        // and grabs composite it exactly as seen.
        const RADIUS: f64 = 2.0;
        self.draw_stroke.stroke_color = vec4(1.0, 0.27, 0.27, 0.9);
        let mut last: Option<(f64, f64)> = None;
        for (x, y) in points.iter().copied() {
            if let Some((lx, ly)) = last {
                let dx = x - lx;
                let dy = y - ly;
                let dist = (dx * dx + dy * dy).sqrt();
                let steps = (dist / (RADIUS * 0.8)).ceil().max(1.0) as usize;
                for step in 1..=steps {
                    let t = step as f64 / steps as f64;
                    let px = lx + dx * t;
                    let py = ly + dy * t;
                    self.draw_stroke.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(px - RADIUS, py - RADIUS),
                            size: dvec2(RADIUS * 2.0, RADIUS * 2.0),
                        },
                    );
                }
            } else {
                self.draw_stroke.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(x - RADIUS, y - RADIUS),
                        size: dvec2(RADIUS * 2.0, RADIUS * 2.0),
                    },
                );
            }
            last = Some((x, y));
        }
    }
}

impl Widget for Tweaker {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !tweak_is_on() {
            return;
        }
        // An eyedropper sample in flight: apply it the moment the frame
        // has been read back, else look again next frame.
        let probe = session().lock().unwrap().eyedrop_probe.clone();
        if let Some((id, prop)) = probe {
            match makepad_platform::pixel_probe::take_pixel_probe(id) {
                Some(Some(rgba)) => {
                    session().lock().unwrap().eyedrop_probe = None;
                    let hex = format_hex(
                        [
                            rgba[0] as f32 / 255.0,
                            rgba[1] as f32 / 255.0,
                            rgba[2] as f32 / 255.0,
                            rgba[3] as f32 / 255.0,
                        ],
                        true,
                    );
                    log!("TWEAK eyedropper {prop} <- {hex}");
                    let sel = session().lock().unwrap().pinned.clone();
                    if let Some(sel) = sel {
                        self.sidebar_apply(cx, &sel, &format!("{prop}: {hex}"));
                        self.rows_uid = 0;
                    }
                    self.redraw_sidebar(cx);
                }
                Some(None) => {
                    self.next_frame = cx.new_next_frame();
                }
                None => {
                    session().lock().unwrap().eyedrop_probe = None;
                }
            }
        }
        if self.live_timer.is_event(event).is_some() {
            self.live_apply(cx);
        }
        // The guard must drop before undo/redo take the session lock again
        // (an `if let` scrutinee's temporary lives for the whole body).
        let pending_undo = session().lock().unwrap().undo_redo.take();
        if let Some(undo) = pending_undo {
            if undo {
                self.undo(cx);
            } else {
                self.redo(cx);
            }
        }
        let theme_req = session().lock().unwrap().theme_req.take();
        if let Some((name, value)) = theme_req {
            if let Err(error) = self.theme_set(cx, &name, &value, "remote") {
                log!("TWEAK theme set failed: {error}");
            }
        }
        let pulse_req = session().lock().unwrap().pulse_req.take();
        if let Some(req) = pulse_req {
            let color = if req.is_empty() {
                None
            } else if let Some(hex) = req.strip_prefix('#') {
                u32::from_str_radix(hex, 16).ok().map(|v| if hex.len() == 6 { (v << 8) | 0xff } else { v })
            } else {
                let found = self.theme_colors.iter().find(|(n, _, _)| *n == req).map(|(_, c, _)| *c);
                if found.is_none() {
                    log!("TWEAK pulse: unknown theme colour {req}");
                }
                found
            };
            self.pulse_pinned = color.is_some();
            self.set_pulse(cx, color);
        }
        if let Event::MouseMove(e) = event {
            self.doc_tip_hover(cx, e.abs);
            self.scope_tip_hover(cx, e.abs);
            self.chrome_tip_hover(cx, e.abs);
            self.states_hover(cx, e.abs);
            self.pulse_hover(cx, e.abs);
        }
        if self.pulse_frame.is_event(event).is_some() {
            if let Some((_, t0)) = self.pulse {
                let now = cx.seconds_since_app_start();
                let t = now - t0;
                let lock = session().lock().unwrap().pulse_lock;
                let m = lock.unwrap_or_else(|| 0.3 + 0.25 * ((t * std::f64::consts::TAU * 2.5).sin() as f32));
                // Frames arrive far faster than the display on a hidden or
                // occluded window; pace the buffer work at ~90 Hz, and a
                // locked tone needs no work at all once it is on screen.
                let due = now - self.pulse_last_sync >= 1.0 / 90.0;
                let mut st = if due { session().lock().unwrap().pulse.take() } else { None };
                if st.as_ref().is_some_and(|s| lock.is_some() && s.m == m && self.pulse_ticks > 0) {
                    session().lock().unwrap().pulse = st.take();
                }
                if let Some(mut st) = st {
                    self.pulse_last_sync = now;
                    st.m = m;
                    let hits = pulse_sync(cx, &mut st);
                    pulse_repaint(cx, &st);
                    if self.pulse_ticks == 0 {
                        let n = |f: &dyn Fn(&PulseSlot) -> bool| st.ledger.iter().filter(|s| f(s)).count();
                        log!(
                            "TWEAK pulse hits {hits} (inst {} uni {} scope {} clear {})",
                            n(&|s| matches!(s, PulseSlot::Inst { .. })),
                            n(&|s| matches!(s, PulseSlot::Uni { .. })),
                            n(&|s| matches!(s, PulseSlot::Scope { .. })),
                            n(&|s| matches!(s, PulseSlot::Clear { .. }))
                        );
                    } else if self.pulse_ticks % 90 == 0 {
                        log!("TWEAK pulse tick {} m={m:.2} ledger {hits}", self.pulse_ticks);
                    }
                    session().lock().unwrap().pulse = Some(st);
                    self.pulse_ticks += 1;
                }
                self.pulse_frame = cx.new_next_frame();
            }
        }
        // The wells' scroll viewports, read between frames when the rect
        // slots hold the drawn values (mid-draw they answer zero).
        if tweak_is_on() {
            if let Some(sidebar) = self.sidebar.clone() {
                let col_rect = sidebar.child(live_id!(shader_col)).area().rect(cx);
                if col_rect.size.y > 0.0 {
                    self.swatch_clip = Some(col_rect);
                }
                let wrap = sidebar.child(live_id!(props_wrap));
                let wrap_rect = wrap.area().rect(cx);
                if wrap_rect.size.y > 0.0 {
                    let scope = wrap.child(live_id!(scope_row)).area().rect(cx);
                    let top = if scope.size.y > 0.0 { scope.pos.y + scope.size.y } else { wrap_rect.pos.y };
                    self.props_viewport = Some(Rect {
                        pos: dvec2(wrap_rect.pos.x, top),
                        size: dvec2(wrap_rect.size.x, (wrap_rect.pos.y + wrap_rect.size.y - top).max(0.0)),
                    });
                }
            }
        }
        if self.states_frame.is_event(event).is_some() {
            self.states_tick(cx);
        }
        if self.live_error_pending && self.next_frame.is_event(event).is_some() {
            // The backend compile happens at draw: an error there shows up
            // one frame after the apply.
            self.live_error_pending = false;
            if let Some(err) = makepad_platform::shader_error::take() {
                self.live_revert(cx, &err);
            }
        }
        if self.swatch_refresh && self.next_frame.is_event(event).is_some() {
            self.swatch_refresh = false;
            self.redraw_sidebar(cx);
        }
        // The suppression window expired: bring the solid outline back.
        if self.next_frame.is_event(event).is_some() {
            let (until, hold) = {
                let s = session().lock().unwrap();
                (s.suppress_until, s.edit_hold)
            };
            let now = cx.seconds_since_app_start();
            // The footer's "path copied" receipt has had its beat: put the
            // path back. (Frames keep arriving until then because the draw
            // asks for one while the receipt is up.)
            if self.footer_copied_until > 0.0 && now >= self.footer_copied_until {
                self.footer_copied_until = 0.0;
                self.redraw_sidebar(cx);
            }
            if now < until || hold {
                self.next_frame = cx.new_next_frame();
            } else {
                self.redraw_overlay(cx);
            }
        }
        // Picking arrives pre-resolved through `window_intercept`; here the
        // tweaker handles its own chrome: the splitter, the sidebar, and
        // the fold / double-click-reset gestures on its rows.
        match event {
            Event::MouseDown(e) if Some(e.window_id.id()) == self.my_window => {
                let x = self.band.pos.x;
                let grip = self.spec_grip_hit(cx, e.abs);
                if e.abs.x >= x - 3.0
                    && e.abs.x <= x + SPLITTER_WIDTH + 3.0
                    && e.abs.y >= self.band.pos.y
                {
                    self.splitter_drag = true;
                } else if let Some(k) = grip {
                    // Grab where it was grabbed: the boundary moves by how far
                    // the pointer has moved since, not to where it is. The
                    // weights are taken as they are; the screen is measured
                    // ONLY for the exchange rate between a point of travel
                    // and a unit of weight, where a frame of staleness costs
                    // a little speed and no position.
                    let weights = [spec_weight(0), spec_weight(1), spec_weight(2)];
                    // Only the height ABOVE the floors is shared by weight:
                    // the engine reserves each row's minimum first and hands
                    // out what is left in proportion. So the exchange rate
                    // between a point of travel and a unit of weight is
                    // measured against that remainder, not the whole -- with
                    // the whole, a 64-point drag moved the boundary 44.
                    let (mut rows, mut on_screen) = (0.0, 0.0);
                    for index in 0..3 {
                        if let Some((_, bx, _)) = self.spec_row(index) {
                            let h = bx.area().rect(cx).size.y;
                            if h > 0.0 {
                                rows += 1.0;
                                on_screen += h;
                            }
                        }
                    }
                    let free = on_screen - rows * SPEC_FIELD_MIN;
                    if free > 0.0 {
                        let per_point = weights.iter().sum::<f64>() / free;
                        self.spec_resize = Some((k, e.abs.y, weights, per_point));
                    }
                } else if e.abs.x > x && self.footer_path_hit(cx, e.abs) {
                    // The footer's path line is the selection's ADDRESS, and
                    // it is shown head-clipped because it does not fit. One
                    // click puts the whole thing on the clipboard, so it can
                    // be pasted into a note, an issue or a prompt as the
                    // unambiguous name of what is selected.
                    self.copy_footer_path(cx);
                } else if e.abs.x > x
                    && !self
                        .open_popup
                        .is_some_and(|rect| rect.contains(e.abs))
                {
                    // Inside the sidebar band: section folds and the
                    // double-click-on-label reset gesture. Row areas are
                    // read here, at event time — they answer for the frame
                    // already on screen. A pointer inside an open popover
                    // belongs to the popover alone.
                    let hit = self
                        .visible
                        .iter()
                        .map(|row| (row.kind, row.item.area().clipped_rect(cx)))
                        .find(|(_, rect)| rect.size.y > 0.0 && rect.contains(e.abs));
                    if let Some((kind, rect)) = hit {
                        match kind {
                            VisKind::Section(section, _, _) => {
                                if self.filter.is_empty() {
                                    let index = section.index();
                                    self.collapsed[index] = !self.collapsed[index];
                                    self.redraw_sidebar(cx);
                                }
                            }
                            VisKind::More(section, _) => {
                                let index = section.index();
                                self.expanded[index] = !self.expanded[index];
                                self.redraw_sidebar(cx);
                            }
                            VisKind::TweakHeader(..) => {
                                self.tweakables_open = !self.tweakables_open;
                                self.redraw_sidebar(cx);
                            }
                            VisKind::Tweakable(_) | VisKind::InputsHeader(_) | VisKind::CascadeLevel(_) => {}
                            VisKind::Material(mi) => {
                                // Thumbnail click: jump to the Shader tab
                                // with this draw layer loaded.
                                if let Some(layer) = self.materials.get(mi) {
                                    self.vibe_layer = Some(layer.clone());
                                    self.panel_tab = PanelTab::Shader;
                                    self.redraw_sidebar(cx);
                                }
                            }
                            VisKind::Size
                            | VisKind::Measured
                            | VisKind::Identity
                            | VisKind::BoxInset(_)
                            | VisKind::FlowSpacing
                            | VisKind::AlignGrid
                            | VisKind::Group(_)
                            | VisKind::Container
                            | VisKind::Absolute
                            | VisKind::GridTracks
                            | VisKind::Cell => {}
                            VisKind::Prop(row_index) => {
                                // The origin-dot zone is the right edge:
                                // click jumps the cascade to that level.
                                let dot_zone = Rect {
                                    pos: dvec2(
                                        rect.pos.x + rect.size.x - 16.0,
                                        rect.pos.y,
                                    ),
                                    size: dvec2(16.0, rect.size.y),
                                };
                                let first = self.rows[row_index]
                                    .prop
                                    .split('.')
                                    .next()
                                    .unwrap_or("")
                                    .to_string();
                                if dot_zone.contains(e.abs)
                                    && self.rows[row_index].section != SectionKind::Cascade
                                {
                                    if let Some(&level) =
                                        self.origin_levels.get(&first)
                                    {
                                        self.collapsed
                                            [SectionKind::Cascade.index()] = false;
                                        self.scroll_to_cascade_level(cx, level);
                                        self.redraw_sidebar(cx);
                                        return;
                                    }
                                }
                                // The label zone is the row's left column.
                                let label_zone = Rect {
                                    pos: rect.pos,
                                    size: dvec2(
                                        104.0_f64.min(rect.size.x * 0.5),
                                        rect.size.y,
                                    ),
                                };
                                if label_zone.contains(e.abs) {
                                    let now = e.time;
                                    let double = self
                                        .last_label_click
                                        .is_some_and(|(t, row)| {
                                            row == row_index && now - t < 0.4
                                        });
                                    if double {
                                        self.last_label_click = None;
                                        let sel = session().lock().unwrap().pinned.clone();
                                        if let Some(sel) = sel {
                                            self.reset_row(cx, &sel, row_index);
                                        }
                                    } else {
                                        self.last_label_click = Some((now, row_index));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Event::KeyDown(ke)
                if ke.key_code == KeyCode::ReturnKey
                    && (ke.modifiers.control || ke.modifiers.logo)
                    && tweak_is_on()
                    && self.panel_tab == PanelTab::Shader
                    && !self.prompt_field_focused(cx) =>
            {
                let text = self
                    .sidebar
                    .as_ref()
                    .map(|s| {
                        s.child(live_id!(shader_col))
                            .child(live_id!(src_scroll))
                            .child(live_id!(shader_src))
                            .text()
                    })
                    .unwrap_or_default();
                // The strip owns Ctrl+Enter when IT has focus (the AI loop).
                // That is settled in this arm's guard rather than here, so
                // that the strip's own arm -- which comes later in the match
                // -- actually receives the key; the editor's edit applies
                // otherwise.
                if text.contains("fn") {
                    if let Err(error) = apply_fn_edit(cx, self, &text) {
                        self.live_revert(cx, &error);
                    } else {
                        self.live_last_applied = text.clone();
                        self.live_last_good = text;
                    }
                }
            }
            // The note hotkey. Insert is the natural key and stays bound,
            // but half the keyboards in use (laptops, 60% boards) reach it
            // only through Fn — so Ctrl+Shift+N (Cmd+Shift+N on mac) opens
            // the same card, and unlike a bare key it also works while the
            // caret sits in one of the panel's fields.
            Event::KeyDown(ke)
                if tweak_is_on()
                    && (ke.key_code == KeyCode::Insert
                        || (ke.key_code == KeyCode::KeyN
                            && ke.modifiers.shift
                            && (ke.modifiers.control || ke.modifiers.logo))) =>
            {
                self.toggle_note(cx);
            }
            _ if self.note_request && tweak_is_on() => {
                self.note_request = false;
                self.toggle_note(cx);
            }
            _ if self.view_focus_pending && tweak_is_on() => {
                self.view_focus_pending = false;
                self.apply_view_focus(cx);
                self.redraw_sidebar(cx);
            }
            // A pin badge was clicked: select its widget and open its note.
            _ if self.badge_open.is_some() && tweak_is_on() => {
                let uid = self.badge_open.take().unwrap();
                self.open_badged_note(cx, uid);
            }
            // Ctrl+Enter in the prompt strip: send the queue. The field
            // would otherwise take Return, so this arm runs before it.
            Event::KeyDown(ke)
                if tweak_is_on()
                    && matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::NumpadEnter)
                    && (ke.modifiers.control || ke.modifiers.logo)
                    && self.prompt_field_focused(cx) =>
            {
                self.prompt_send(cx);
            }
            // Alt+Enter queues instead of sending: write against several
            // widgets first, then release the batch with one Ctrl+Enter.
            Event::KeyDown(ke)
                if tweak_is_on()
                    && matches!(ke.key_code, KeyCode::ReturnKey | KeyCode::NumpadEnter)
                    && ke.modifiers.alt
                    && !ke.modifiers.control
                    && !ke.modifiers.logo
                    && self.prompt_field_focused(cx) =>
            {
                self.prompt_queue(cx);
            }
            // Up / Down in an EMPTY box walks the history. Only while empty:
            // in a message being written those keys belong to the caret.
            Event::KeyDown(ke)
                if tweak_is_on()
                    && matches!(ke.key_code, KeyCode::ArrowUp | KeyCode::ArrowDown)
                    && !ke.modifiers.any()
                    && self.prompt_field_focused(cx)
                    && self.prompt_field_empty() =>
            {
                self.prompt_recall(cx, ke.key_code == KeyCode::ArrowUp);
            }
            // Escape calls off an armed @mention first — something is still
            // being written in — and only leaves the tab once there is no
            // pick outstanding.
            Event::KeyDown(ke)
                if self.panel_tab == PanelTab::Spec
                    && tweak_is_on()
                    && ke.key_code == KeyCode::Escape =>
            {
                if session().lock().unwrap().mention {
                    session().lock().unwrap().mention = false;
                    self.redraw_overlay(cx);
                } else {
                    self.note_close(cx);
                }
            }
            // The arrows walk the hierarchy while something is selected —
            // parent / first child / previous / next sibling, the scene
            // editor's vocabulary. With NOTHING selected they belong to the
            // exploded view's orbit (platform/src/sploded.rs stands down
            // for exactly this case), and with a caret anywhere they belong
            // to the caret.
            Event::KeyDown(ke)
                if tweak_is_on()
                    && matches!(
                        ke.key_code,
                        KeyCode::ArrowUp
                            | KeyCode::ArrowDown
                            | KeyCode::ArrowLeft
                            | KeyCode::ArrowRight
                    )
                    && !self.focus_is_text(cx)
                    && session().lock().unwrap().pinned.is_some() =>
            {
                self.walk_selection(cx, ke.key_code);
            }
            Event::KeyDown(ke)
                if ke.key_code == KeyCode::KeyZ
                    && ke.modifiers.logo
                    && tweak_is_on()
                    && cx.key_focus() == Area::Empty =>
            {
                if ke.modifiers.shift {
                    self.redo(cx);
                } else {
                    self.undo(cx);
                }
            }
            Event::KeyDown(ke)
                if ke.key_code == KeyCode::Slash
                    && tweak_is_on()
                    && cx.key_focus() == Area::Empty =>
            {
                // '/': jump to the filter (only when nothing is editing).
                self.focus_search_pending = true;
                self.redraw_sidebar(cx);
            }
            Event::Scroll(_) if tweak_is_on() => {
                // Content may have moved under the outlines: repaint the
                // overlay with the freshly-read rects.
                self.redraw_overlay(cx);
            }
            // A field is being resized: its height follows the pointer from
            // where the strip was grabbed.
            Event::MouseMove(e) if self.spec_resize.is_some() => {
                let (k, from_y, from, per_point) = self.spec_resize.unwrap();
                // The pair either side of the boundary trade weight between
                // them; their sum is fixed, and neither may go below the
                // floor -- which is what stops one growing forever. The
                // third row is not touched, so it keeps exactly its share.
                // A weight of zero IS the floor -- the engine's reserved
                // minimum is all that row then gets -- so the clamp is on
                // the pair's weight itself, not on some pixel figure.
                let pair = from[k] + from[k + 1];
                let above = (from[k] + (e.abs.y - from_y) * per_point).clamp(0.0, pair);
                let mut weights = from;
                weights[k] = above;
                weights[k + 1] = pair - above;
                session().lock().unwrap().spec_weights = weights;
                // Applied here, at event time, so the next layout pass is
                // already the new split rather than the one after it.
                self.spec_apply_weights(cx);
                cx.set_cursor(MouseCursor::NsResize);
                self.redraw_sidebar(cx);
            }
            // The strip says what it is before it is grabbed.
            Event::MouseMove(e)
                if Some(e.window_id.id()) == self.my_window
                    && tweak_is_on()
                    && self.spec_grip_hit(cx, e.abs).is_some() =>
            {
                cx.set_cursor(MouseCursor::NsResize);
            }
            Event::MouseMove(e)
                if Some(e.window_id.id()) == self.my_window
                    && !self.splitter_drag
                    && tweak_is_on() =>
            {
                // The footer's path line copies on click, so it says so
                // with the hand before it is clicked.
                if self.footer_path_hit(cx, e.abs)
                    && session().lock().unwrap().pinned.is_some()
                {
                    cx.set_cursor(MouseCursor::Hand);
                }
                // The doc tooltip: a row whose prop carries doc-channel
                // text shows it, anchored to the row (no per-pixel churn).
                // Tree tab: hovering a row outlines its widget in the body.
                if self.panel_tab == PanelTab::Tree
                    && self.band.size.x > 0.0
                    && e.abs.x > self.band.pos.x
                {
                    let mut hover_target = None;
                    for (item, target) in &self.tree_visible {
                        let rect = item.area().clipped_rect_union(cx);
                        if rect.size.y > 0.0 && rect.contains(e.abs) {
                            hover_target = Some(*target);
                            break;
                        }
                    }
                    match hover_target {
                        Some(target) => {
                            let widget = cx.widget_tree().widget(WidgetUid(target));
                            if !widget.is_empty() {
                                let rect = widget.area().clipped_rect_union(cx);
                                if rect.size.x > 0.0 {
                                    session().lock().unwrap().hover = Some(TweakPick {
                                        uid: target,
                                        path: String::new(),
                                        ty: String::new(),
                                        rect,
                                        window_id: self.my_window.unwrap_or(0),
                                        band: None,
                                        level: 0,
                                    });
                                    self.tree_hover_active = true;
                                    self.redraw_overlay(cx);
                                }
                            }
                        }
                        None => {
                            if self.tree_hover_active {
                                self.tree_hover_active = false;
                                session().lock().unwrap().hover = None;
                                self.redraw_overlay(cx);
                            }
                        }
                    }
                }
                let mut new = None;
                if self.band.size.x > 0.0
                    && e.abs.x > self.band.pos.x
                    && !self
                        .open_popup
                        .is_some_and(|rect| rect.contains(e.abs))
                {
                    let hit = self
                        .visible
                        .iter()
                        .map(|row| (row.kind, row.item.area().clipped_rect(cx)))
                        .find(|(_, rect)| rect.size.y > 0.0 && rect.contains(e.abs));
                    if let Some((VisKind::Prop(index), rect)) = hit {
                        if let Some(doc) = self.row_docs.get(&self.rows[index].prop) {
                            let mut line = doc.lines().next().unwrap_or("").to_string();
                            if doc.lines().count() > 1 {
                                line.push_str(" \u{2026}");
                            }
                            new = Some(HoverDoc {
                                text: line,
                                pos: dvec2(rect.pos.x, rect.pos.y - 18.0),
                            });
                        }
                    }
                }
                if new != self.hover_doc {
                    self.hover_doc = new;
                    self.redraw_sidebar(cx);
                }
            }
            Event::MouseMove(e) if self.splitter_drag => {
                let window_right = self.band.pos.x + self.band.size.x;
                let width = (window_right - e.abs.x).clamp(180.0, 560.0);
                session().lock().unwrap().sidebar_width = width;
                cx.set_cursor(MouseCursor::ColResize);
                cx.redraw_all();
            }
            // ANY up releases the drag, wherever it lands — the capture
            // must never outlive the press.
            Event::MouseUp(_) => {
                self.splitter_drag = false;
                self.spec_resize = None;
            }
            // Safeties: focus loss or Escape frees the pointer too.
            Event::WindowLostFocus(_) => {
                self.splitter_drag = false;
                self.spec_resize = None;
            }
            Event::KeyDown(ke)
                if ke.key_code == KeyCode::Escape
                    && (self.splitter_drag || self.spec_resize.is_some()) =>
            {
                self.splitter_drag = false;
                self.spec_resize = None;
            }
            _ => {}
        }
        // While a popover is open, wheel inside its rect must not scroll
        // the property list underneath it.
        let swallow_scroll = matches!(event, Event::Scroll(e)
            if self.open_popup.is_some_and(|rect| rect.contains(e.abs)));
        // The popover draws above the list but its row is last in event
        // order, so the rows underneath claimed hovers and presses first
        // (first claimant wins). Pointer events inside the popover go to
        // its row before the list; the list's pass then finds them handled.
        let pointer = match event {
            Event::MouseMove(e) => Some(e.abs),
            Event::MouseDown(e) => Some(e.abs),
            Event::MouseUp(e) => Some(e.abs),
            _ => None,
        };
        if let Some(abs) = pointer {
            if self.open_popup.is_some_and(|rect| rect.contains(abs)) {
                let owner = self
                    .visible
                    .iter()
                    .find(|v| {
                        v.item
                            .child(live_id!(swatch))
                            .borrow::<FabColorPick>()
                            .is_some_and(|p| p.is_open())
                    })
                    .map(|v| v.item.clone());
                if let Some(item) = owner {
                    item.handle_event(cx, event, scope);
                }
            }
        }
        if let Some(sidebar) = self.sidebar.clone() {
            if !swallow_scroll {
                sidebar.handle_event(cx, event, scope);
            }
        }
        // The floating extrusion readout takes its own input, whether or not
        // a note card happens to be open — it was nested inside the note's
        // block, so the field only answered while a note was up.
        if let Some(ui) = self.spread_ui.clone() {
            ui.handle_event(cx, event, scope);
        }
        if let Event::Actions(actions) = event {
            self.handle_sidebar_actions(cx, actions);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let on = tweak_is_on();
        if on && !self.was_on {
            // Opening the panel lands the caret in the filter.
            self.focus_search_pending = true;
        }
        if self.view_zoom < 1.0 {
            self.view_zoom = 1.0;
        }
        self.was_on = on;
        // A theme edit made outside this overlay (a tool going through
        // `reflect::theme_set_value`) bumps the session's apply generation;
        // the palette strip and the swatch names follow it here instead of
        // keeping the values they were read with.
        let apply_gen = session().lock().unwrap().apply_gen;
        if !self.theme_colors.is_empty() && self.palette_gen != apply_gen {
            self.theme_colors = theme_palette(cx);
            self.palette_gen = apply_gen;
        }
        let window_id = cx.get_current_window_id().map(|id| id.id());
        self.my_window = window_id;
        // Compress the app's UI while the sidebar is up; release it when the
        // mode goes off (this draw still runs once after the toggle because
        // set_tweak_on redraws everything).
        let desired = if on { sidebar_width() } else { 0.0 };
        self.ensure_body_margin(cx, desired);
        if !on {
            // Tear the surface down for real: both overlay lists are
            // RETAINED by the window's overlay (a stored sub-list keeps its
            // slot and its last items), so skipping them here left the
            // panel, the outlines and the note card painted after Shift+F10.
            // Begin and end them empty so nothing of the mode remains.
            // The design surface is going away, so the app must go back to
            // life size and its own centre with it.
            if cx.sploded_focus() != (None, 0.0, 1.0) {
                cx.sploded_set_focus(None, 0.0, 1.0);
            }
            let mut hands_off = false;
            for (index, list) in [self.overlay_list.as_mut(), self.sidebar_list.as_mut()]
                .into_iter()
                .flatten()
                .enumerate()
            {
                list.begin_overlay_reuse(cx);
                let size = cx.current_pass_size();
                cx.begin_root_turtle(size, Layout::flow_down());
                // The mode is off, but the bridge may still be driving: the
                // frame that says so is drawn into the topmost list.
                if index == 1 && draw_hands_off_frame(cx, &mut self.draw_outline) {
                    hands_off = true;
                }
                cx.end_pass_sized_turtle();
                list.end(cx);
            }
            if hands_off {
                self.next_frame = cx.new_next_frame();
            }
            return DrawStep::done();
        }
        let (hover, pinned, strokes, live_stroke, suppress_until, edit_hold) = {
            let s = session().lock().unwrap();
            (
                s.hover.clone(),
                s.pinned.clone(),
                s.strokes.clone(),
                s.live_stroke.clone(),
                s.suppress_until,
                s.edit_hold,
            )
        };
        // While a value is actively moving (field focused / scrubbed, color
        // popover open, an apply within the last beat, a handle drag) the
        // pinned outline yields to a faint hairline stipple so the widget is
        // judged exactly as it renders.
        let now = cx.seconds_since_app_start();
        let quiet = edit_hold || now < suppress_until || self.radius_drag.is_some();
        if now < suppress_until {
            // Re-check when the linger expires.
            self.next_frame = cx.new_next_frame();
        }

        // Rebuild rows before the overlay: the radius handles need to know
        // whether the selection exposes a radius input.
        let sel = pinned.clone();
        let apply_gen = session().lock().unwrap().apply_gen;
        if self.panel_tab == PanelTab::Theme {
            if self.rows_uid != THEME_ROWS || self.rows_gen != apply_gen {
                self.rebuild_theme_rows(cx);
                self.rows_gen = apply_gen;
                self.swatch_refresh = true;
                self.next_frame = cx.new_next_frame();
            }
        } else if let Some(sel_pick) = &sel {
            if self.rows_uid != sel_pick.uid || self.rows_gen != apply_gen {
                let path = sel_pick.path.clone();
                self.rebuild_rows(cx, sel_pick.uid, &path);
                self.rows_gen = apply_gen;
                self.swatch_refresh = true;
                self.next_frame = cx.new_next_frame();
            }
        } else {
            self.rows.clear();
            self.rows_uid = 0;
            self.radius_prop = None;
        }

        let overlay_list = self.overlay_list.as_mut().unwrap();
        overlay_list.begin_overlay_reuse(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());

        // Selection/hover rects go STALE when containers scroll: re-read
        // the live widget areas every overlay frame (and write back, so
        // /tweak/state reports where things actually are).
        let pinned = pinned.map(|mut pick| {
            let live = cx.widget_tree().widget(WidgetUid(pick.uid));
            if !live.is_empty() {
                let rect = live_rect(cx, &live);
                if rect.size.x > 0.0 && rect != pick.rect {
                    pick.rect = rect;
                    if let Some(pinned) = session().lock().unwrap().pinned.as_mut() {
                        pinned.rect = rect;
                    }
                } else if rect.size.x <= 0.0 {
                    // Not drawn this frame (another tab is up): the pin
                    // stands, but there is nothing on screen to outline.
                    pick.rect = Rect::default();
                }
            }
            pick
        });
        let hover = hover.map(|mut pick| {
            let live = cx.widget_tree().widget(WidgetUid(pick.uid));
            if !live.is_empty() {
                let rect = live_rect(cx, &live);
                pick.rect = if rect.size.x > 0.0 { rect } else { Rect::default() };
            }
            pick
        });
        // Centre TRACKS. The subject moves — the selection changes, a list
        // scrolls, the window resizes — and a centre computed once at the
        // click would hold the middle of the screen on wherever the widget
        // happened to be then. Recompute against the live rects and hand it
        // to the next frame; applying it here would redraw from inside a
        // draw.
        if self.view_center {
            let want = self.focus_point(cx);
            let (have, have_level, _) = cx.sploded_focus();
            let moved = match (want, have) {
                (Some((a, level)), Some(b)) => {
                    (a.x - b.x).abs() > 0.5
                        || (a.y - b.y).abs() > 0.5
                        || (level - have_level).abs() > 1.0e-4
                }
                (a, b) => a.is_some() != b.is_some(),
            };
            if moved {
                self.view_focus_pending = true;
                self.next_frame = cx.new_next_frame();
            }
        }
        // Whether the arrows orbit the exploded view or walk the hierarchy
        // turns on this, so it is reported every frame and independently of
        // whether the selection happens to be drawing.
        cx.sploded_set_selected(pinned.is_some());
        // Exploded view: the outlines belong on their widgets' planes inside
        // the body pass, not flat on the window pass — hand them to the
        // pass owner as marks and draw nothing here.
        if cx.sploded_active() {
            let max_level = cx.sploded_max_level();
            let mark = |cx: &mut Cx, pick: &TweakPick| {
                if Some(pick.window_id) != window_id || pick.rect.size.x <= 0.0 {
                    return None;
                }
                let level = cx.sploded_depth_of(pick.uid).unwrap_or(pick.level);
                let _ = max_level;
                Some(makepad_platform::sploded::SplodedMark { rect: pick.rect, level: level as f32 })
            };
            let hover_mark = if quiet { None } else { hover.as_ref().and_then(|p| mark(cx, p)) };
            let pinned_mark = pinned.as_ref().and_then(|p| mark(cx, p));
            cx.sploded_set_marks(hover_mark, pinned_mark);
        }
        // Isolate covers the app around the selection, under every mark the
        // overlay draws — the marks belong on top of the isolated widget, not
        // under the cover.
        if self.tree_isolate && self.isolate_uid != 0 {
            let widget = cx.widget_tree().widget(WidgetUid(self.isolate_uid));
            let rect = if widget.is_empty() { Rect::default() } else { live_rect(cx, &widget) };
            if rect.size.x > 0.0 && rect.size.y > 0.0 {
                let corners = [
                    rect.pos,
                    dvec2(rect.pos.x + rect.size.x, rect.pos.y),
                    dvec2(rect.pos.x + rect.size.x, rect.pos.y + rect.size.y),
                    dvec2(rect.pos.x, rect.pos.y + rect.size.y),
                ];
                // Exploded: the widget is on a plane, so the hole is where
                // that plane puts it, not where the flat layout does.
                let level = cx.sploded_depth_of(self.isolate_uid).unwrap_or(0) as f32;
                let pass = cx.current_pass_size();
                let hole = corners
                    .map(|p| cx.sploded_project(pass, p, level).unwrap_or(p));
                self.draw_isolate_scrim(cx, hole);
            }
        }
        // NOT an early return: the overlay list and its root turtle were
        // begun above and are ended below — leaving them open let the
        // window's deferred Fill walk resolve against this turtle instead
        // of its own (an index-out-of-bounds in `resolve_fill`).
        // Everything below draws flat on the window pass, so from here the
        // rects are SCREEN rects: the outline, the handles hanging off it,
        // the note card that rides the selection and its leader line all
        // follow the widget through a centre or a zoom instead of staying
        // behind at the layout coordinates.
        let pinned = pinned.map(|mut pick| {
            pick.rect = self.screen_rect(cx, pick.rect);
            pick
        });
        let hover = hover.map(|mut pick| {
            pick.rect = self.screen_rect(cx, pick.rect);
            pick
        });
        let flat_outlines = !cx.sploded_active();
        if flat_outlines {
        if let Some(pick) = &pinned {
            if Some(pick.window_id) == window_id {
                let style = if quiet {
                    PickStyle::PinnedQuiet
                } else {
                    PickStyle::Pinned
                };
                self.draw_pick(cx, pick, style);
                // Direct-manipulation handles, HOVER-REVEALED: the radius
                // dots exist for the hand, not the eye — parked on the
                // selection's corners they read as chrome and hide the very
                // pixels being judged. They appear when the pointer comes
                // within reach of a corner and vanish with it.
                // ...and they stand down while the view is centred or
                // zoomed: the handle is grabbed in layout coordinates and
                // painted in screen ones, so under a transform it would
                // answer to a place it is not.
                if self.radius_prop.is_some() && !cx.sploded_transformed() {
                    let pointer = session().lock().unwrap().pointer_abs;
                    let near = Self::radius_handle_centers(pick.rect).iter().any(|c| {
                        let dx = pointer.x - c.x;
                        let dy = pointer.y - c.y;
                        dx * dx + dy * dy <= 28.0 * 28.0
                    });
                    if near {
                        self.draw_radius_handles(cx, pick.rect);
                    }
                }
            }
        }
        let mention = session().lock().unwrap().mention;
        if !quiet || mention {
            if let Some(pick) = &hover {
                let same = pinned.as_ref().is_some_and(|p| p.uid == pick.uid);
                // An armed mention outlines whatever is under the pointer,
                // the current selection included: naming the widget the note
                // is already on is a legitimate thing to want.
                if Some(pick.window_id) == window_id && (!same || mention) {
                    let style = if mention { PickStyle::Mention } else { PickStyle::Hover };
                    self.draw_pick(cx, pick, style);
                }
            }
        }
        }
        for stroke in &strokes {
            if Some(stroke.window_id) == window_id {
                self.draw_stroke_points(cx, &stroke.points);
            }
        }
        if let Some(stroke) = &live_stroke {
            if Some(stroke.window_id) == window_id {
                self.draw_stroke_points(cx, &stroke.points);
            }
        }
        // Pin badges: every widget carrying a PINNED note wears one, so old
        // notes announce themselves. Drawn before the card, which may cover
        // one of them.
        // Note mode: while a card is open, every widget carrying a pinned
        // note wears a pin, so the others announce themselves and can be
        // opened with a click. Outside note mode they would be chrome on the
        // canvas answering a question nobody asked.
        if flat_outlines && self.panel_tab == PanelTab::Spec {
            self.refresh_badges(cx);
            self.draw_badges(cx, scope, window_id);
        } else {
            self.badge_rects.clear();
        }

        // The extrusion readout, top-right of the APP — not the panel. It
        // is the one control the eye needs while it is on the stack, and
        // crossing the window to a sidebar field to reach it meant looking
        // away from the thing being adjusted.
        self.spread_rect = None;
        if cx.sploded_will_be_active() {
            self.ensure_spread_ui(cx);
            let ui = self.spread_ui.as_ref().unwrap().clone();
            {
                let field = ui.child(live_id!(value));
                let live = cx.sploded_spread() as f64;
                if field.area() == Area::Empty || !cx.has_key_focus(field.area()) {
                    if let Some(mut field) = field.borrow_mut::<FabValueInput>() {
                        field.set_value(cx, live);
                    }
                }
            }
            const HUD_W: f64 = 116.0;
            let max_x = self.overlay_max_x(cx.current_pass_size());
            let mut walk = Walk::fit();
            walk.abs_pos = Some(dvec2((max_x - HUD_W - 8.0).max(0.0), 8.0));
            walk.width = Size::Fixed(HUD_W);
            let _ = ui.draw_walk(cx, scope, walk);
            let rect = ui.area().rect(cx);
            if rect.size.x > 0.0 {
                self.spread_rect = Some(rect);
            }
        }

        // The extrusion readout is drawn FLAT on the window pass, so in the
        // exploded view the mode must not re-address the pointer over it: it
        // would be painted in one place and clicked in another. Same
        // exemption the panel band has, but this one moves.
        let floating: Vec<Rect> = self.spread_rect.into_iter().collect();
        cx.sploded_set_flat_rects(floating.clone());
        session().lock().unwrap().chrome_float = floating;

        cx.end_pass_sized_turtle();
        self.overlay_list.as_mut().unwrap().end(cx);

        // The property sidebar in its own overlay list, begun after the
        // outline overlay: the app (dock tab bars included) stacks below
        // it, and popups the panel opens (color pickers) begin later still,
        // so they stack above.
        let sidebar_list = self.sidebar_list.as_mut().unwrap();
        sidebar_list.begin_overlay_reuse(cx);
        let size = cx.current_pass_size();
        cx.begin_root_turtle(size, Layout::flow_down());
        self.draw_sidebar(cx, scope, sel.as_ref());
        // The caret goes back into the prompt box AFTER the sidebar has been
        // drawn, not while it is being laid out: before the draw the box's
        // area is last frame's, and the focus set against it does not
        // survive the field being drawn again. Without this a send or a
        // queue drops the caret, and every key after it is handled by the
        // APP -- typing walks its tabs instead of writing the next message.
        if self.prompt_focus_pending {
            let field = self
                .sidebar
                .as_ref()
                .map(|s| s.child(live_id!(prompt_row)).child(live_id!(prompt_field)));
            if let Some(area) = field.map(|f| f.area()).filter(|a| *a != Area::Empty) {
                self.prompt_focus_pending = false;
                cx.set_key_focus(area);
                self.next_frame = cx.new_next_frame();
            }
        }
        // The doc chip for whatever the pointer is over -- a property row's
        // annotation, or one of the panel's own controls. Drawn HERE, after
        // the sidebar, rather than inside its draw: painted in there, every
        // widget the panel drew afterwards lay on top of it, and a chip under
        // the tab row was a dark plate with the tabs showing through.
        if let Some(hover) = self.hover_doc.clone() {
            let band = self.band;
            let label_height = 16.0;
            // MEASURED, not counted: the words are drawn at their true
            // advances, so the plate is sized from the same run. The count
            // is only the fallback for text the layout engine returns no
            // row for.
            let approx = self
                .draw_label
                .prepare_single_line_run(cx, &hover.text)
                .map(|run| run.width_in_lpxs as f64)
                .unwrap_or_else(|| (hover.text.chars().count() as f64) * 5.4)
                + 10.0;
            let mut pos = hover.pos;
            pos.x = pos
                .x
                .clamp(band.pos.x, (band.pos.x + band.size.x - approx).max(band.pos.x));
            pos.y = pos.y.max(0.0);
            self.draw_label_bg.draw_abs(cx, Rect { pos, size: dvec2(approx, label_height) });
            self.draw_label.draw_abs(cx, pos + dvec2(5.0, 2.0), &hover.text);
        }
        // Last into the topmost list, so it lies over the panel too.
        if draw_hands_off_frame(cx, &mut self.draw_outline) {
            self.next_frame = cx.new_next_frame();
        }
        cx.end_pass_sized_turtle();
        self.sidebar_list.as_mut().unwrap().end(cx);

        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_row_reads_as_one_value_however_it_was_spelled() {
        assert_eq!(
            parse_size_text("Size.Fill{weight: 100 basis: FitBound.Abs(0) shrink: 0}"),
            SizeText::Fill(FillText::plain())
        );
        assert_eq!(parse_size_text("Fill"), SizeText::Fill(FillText::plain()));
        assert_eq!(
            parse_size_text("Size.Fill{weight: 2 shrink: 1}"),
            SizeText::Fill(FillText { weight: 2.0, shrink: 1.0, ..FillText::plain() })
        );
        assert_eq!(parse_size_text("Size.Fit{}"), SizeText::Fit);
        assert_eq!(parse_size_text("Fit"), SizeText::Fit);
        assert_eq!(parse_size_text("Size.Fixed(200)"), SizeText::Fixed(200.0));
        assert_eq!(parse_size_text("200"), SizeText::Fixed(200.0));
        assert_eq!(parse_size_text("24px"), SizeText::Fixed(24.0));
        assert_eq!(parse_size_text("\"50%\""), SizeText::Rel("50%".into()));
        assert_eq!(parse_size_text("25vw"), SizeText::Rel("25vw".into()));
        assert_eq!(parse_size_text("calc(100% - 20px)"), SizeText::Expr("calc(100% - 20px)".into()));
        assert_eq!(parse_size_text(""), SizeText::Fit);
    }

    #[test]
    fn a_fill_keeps_every_field_it_had_and_writes_back_whole() {
        let fill = parse_fill_text("Size.Fill{weight: 2 basis: 25% shrink: 1 min: 48 max: 200}");
        assert_eq!(fill.basis, "25%");
        assert_eq!(fill.min, Some(48.0));
        assert_eq!(
            fill.chunk("width"),
            "width: Size.Fill{weight: 2 basis: \"25%\" shrink: 1 min: 48 max: 200}"
        );
        let plain = parse_fill_text("Size.Fill{weight: 100 basis: FitBound.Abs(0) shrink: 0}");
        assert_eq!(plain.chunk("height"), "height: Size.Fill{weight: 100 basis: 0 shrink: 0}");
        let expr = parse_fill_text("Size.Fill{weight: 1 basis: \"calc(100% - 20px)\" shrink: 0}");
        assert_eq!(expr.basis, "calc(100% - 20px)");
        assert_eq!(expr.chunk("width"), "width: Size.Fill{weight: 1 basis: \"calc(100% - 20px)\" shrink: 0}");
        assert_eq!(parse_fill_text("Fill").chunk("width"), "width: Size.Fill{weight: 100 basis: 0 shrink: 0}");
    }

    #[test]
    fn a_bound_or_an_aspect_is_set_by_its_text_and_cleared_by_none() {
        assert_eq!(bound_text("FitBound.Abs(120)"), "120");
        assert_eq!(bound_text("50%"), "50%");
        assert_eq!(bound_text("\"calc(100% - 20px)\""), "calc(100% - 20px)");
        assert_eq!(bound_chunk("min_width", "120").as_deref(), Some("min_width: 120"));
        assert_eq!(bound_chunk("max_width", "50%").as_deref(), Some("max_width: \"50%\""));
        assert_eq!(bound_chunk("max_height", "clamp(200px, 50%, 600px)").as_deref(), Some("max_height: \"clamp(200px, 50%, 600px)\""));
        assert_eq!(bound_chunk("min_height", "").as_deref(), Some("min_height: nil"));
        assert_eq!(bound_chunk("min_height", "none").as_deref(), Some("min_height: nil"));
        assert_eq!(bound_chunk("min_height", "Fill"), None);
        assert_eq!(bound_chunk("min_height", "12p"), None);
        assert_eq!(aspect_chunk("1.5").as_deref(), Some("aspect: 1.5"));
        assert_eq!(aspect_chunk("16:9").as_deref(), Some("aspect: 1.7778"));
        assert_eq!(aspect_chunk("3/2").as_deref(), Some("aspect: 1.5"));
        assert_eq!(aspect_chunk("").as_deref(), Some("aspect: nil"));
        assert_eq!(aspect_chunk("3:"), None);
        assert_eq!(aspect_chunk("3:0"), None);
        assert_eq!(aspect_chunk("wide"), None);
    }

    #[test]
    fn a_container_name_is_an_id_and_a_position_is_a_pair() {
        assert_eq!(container_chunk("side").as_deref(), Some("container_id: @side"));
        assert_eq!(container_chunk("@side_2").as_deref(), Some("container_id: @side_2"));
        assert_eq!(container_chunk("2side"), None);
        assert_eq!(container_chunk("side bar"), None);
        assert_eq!(container_chunk(""), None);
        assert_eq!(parse_vec2_text("vec2f(10 20)"), Some((10.0, 20.0)));
        assert_eq!(parse_vec2_text("vec2(10, 20.5)"), Some((10.0, 20.5)));
        assert_eq!(parse_vec2_text("null"), None);
    }

    #[test]
    fn grid_tracks_read_as_css_and_write_back_as_a_list() {
        let printed = "[\"70px\" \"20%\" \"minmax(60px, 1fr)\" \"repeat(2, minmax(50px, 1fr))\"]";
        assert_eq!(quoted_list_text(printed, " "), "70px 20% minmax(60px, 1fr) repeat(2, minmax(50px, 1fr))");
        assert_eq!(quoted_list_text("[]", " "), "");
        assert_eq!(quoted_list_text("[\"hero hero . . .\" \". . . . .\"]", " / "), "hero hero . . . / . . . . .");
        assert_eq!(
            split_tracks("70px 20% minmax(60px, 1fr) repeat(2, minmax(50px, 1fr))"),
            vec!["70px", "20%", "minmax(60px, 1fr)", "repeat(2, minmax(50px, 1fr))"]
        );
        assert_eq!(
            tracks_chunk("columns", "70px 20% minmax(60px, 1fr) repeat(auto-fill, minmax(50px, 1fr))").as_deref(),
            Some("columns: [\"70px\" \"20%\" \"minmax(60px, 1fr)\" \"repeat(auto-fill, minmax(50px, 1fr))\"]")
        );
        assert_eq!(tracks_chunk("rows", "48 1fr").as_deref(), Some("rows: [\"48\" \"1fr\"]"));
        assert_eq!(tracks_chunk("rows", "").as_deref(), Some("rows: []"));
        assert_eq!(tracks_chunk("rows", "1f"), None);
        assert_eq!(tracks_chunk("rows", "minmax(60px"), None);
        assert_eq!(tracks_chunk("rows", "repeat(2, 50px)"), None);
        assert_eq!(tracks_chunk("rows", "calc(100% - 20px)").as_deref(), Some("rows: [\"calc(100% - 20px)\"]"));
        assert_eq!(areas_chunk("hero hero . / . . .").as_deref(), Some("areas: [\"hero hero .\" \". . .\"]"));
        assert_eq!(areas_chunk("").as_deref(), Some("areas: []"));
        assert_eq!(areas_chunk("hero hero . / . ."), None);
    }

    #[test]
    fn a_cell_that_says_nothing_is_no_cell() {
        assert_eq!(CellText::default().chunk(), "cell: nil");
        let placed = CellText { col: 4, row: 1, col_span: 0, row_span: 0, area: String::new() };
        assert_eq!(placed.chunk(), "cell: CellPlacement{col: 4 row: 1 col_span: 0 row_span: 0 area: nil}");
        let named = CellText { area: "hero".to_string(), ..CellText::default() };
        assert_eq!(named.chunk(), "cell: CellPlacement{col: 0 row: 0 col_span: 0 row_span: 0 area: @hero}");
    }

    #[test]
    fn a_flow_row_reads_as_one_value_and_writes_back_whole() {
        let printed = parse_flow_text("Flow.Right{row_align: RowAlign.Top wrap: true}");
        assert_eq!(printed, FlowText { dir: FlowDir::Right, wrap: true, row_align: RowAlignText::Top });
        assert!(printed.wraps());
        assert_eq!(printed.chunk(), "flow: Flow.Right{wrap: true row_align: RowAlign.Top}");
        let centred = parse_flow_text("Flow.Right{wrap: true, row_align: RowAlign.Center}");
        assert_eq!(centred.row_align, RowAlignText::Center);
        assert_eq!(parse_flow_text("Flow.Down").dir, FlowDir::Down);
        assert_eq!(parse_flow_text("Down").chunk(), "flow: Flow.Down");
        assert_eq!(parse_flow_text("Overlay").chunk(), "flow: Flow.Overlay");
        assert_eq!(parse_flow_text("Right").chunk(), "flow: Flow.Right{wrap: false row_align: RowAlign.Top}");
        assert_eq!(parse_flow_text("RightWrap").wraps(), true);
        assert_eq!(parse_flow_text("").dir, FlowDir::Down);
        // Going Down keeps the wrap for the way back; wrap on a Down flow
        // makes it a wrapping Right one; a row alignment implies Right.
        let down = printed.with_dir(FlowDir::Down);
        assert_eq!(down.chunk(), "flow: Flow.Down");
        assert!(!down.wraps());
        assert!(down.toggled_wrap().wraps());
        assert_eq!(down.with_dir(FlowDir::Right).wraps(), true);
        assert!(!printed.toggled_wrap().wraps());
        assert_eq!(
            parse_flow_text("Down").with_row_align(RowAlignText::Bottom).chunk(),
            "flow: Flow.Right{wrap: false row_align: RowAlign.Bottom}"
        );
    }

    #[test]
    fn what_is_typed_into_a_size_field_becomes_the_chunk_the_engine_accepts() {
        assert_eq!(size_chunk("width", "fill").as_deref(), Some("width: Fill"));
        assert_eq!(size_chunk("width", "Fit").as_deref(), Some("width: Fit"));
        assert_eq!(size_chunk("width", "200").as_deref(), Some("width: 200"));
        assert_eq!(size_chunk("height", "24px").as_deref(), Some("height: 24"));
        assert_eq!(size_chunk("width", "50%").as_deref(), Some("width: \"50%\""));
        assert_eq!(size_chunk("width", "clamp(200px, 50%, 600px)").as_deref(), Some("width: \"clamp(200px, 50%, 600px)\""));
        // A Fill with fields still sets Fill; the weight gets its own field.
        assert_eq!(size_chunk("width", "Fill{weight: 2}").as_deref(), Some("width: Fill"));
        // Half a word on its way to being one is not sent.
        assert_eq!(size_chunk("width", "12p"), None);
        assert_eq!(size_chunk("width", "fi"), None);
    }

    #[test]
    fn layout_enums_print_as_whole_values() {
        use crate::makepad_draw::turtle::RowAlign;
        use crate::makepad_draw::{Base, FitBound, Flow, Size};
        use crate::makepad_script::{ScriptApply, ScriptNew};
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            // In dependency order, the way the draw crate's module does it:
            // building a named variant's proto converts its DEFAULT fields
            // to script values, and a bare default such as `Base::Full` or
            // `RowAlign::Top` looks its own type up on the way.
            <Base as ScriptNew>::script_api(vm);
            <RowAlign as ScriptNew>::script_api(vm);
            <FitBound as ScriptNew>::script_api(vm);
            <Size as ScriptNew>::script_api(vm);
            <Flow as ScriptNew>::script_api(vm);
            let fill = Size::Fill {
                weight: 100.0,
                basis: FitBound::Abs(0.0),
                shrink: 0.0,
                min: None,
                max: None,
            }
            .script_to_value(vm);
            let fixed = Size::Fixed(200.0).script_to_value(vm);
            let down = Flow::Down.script_to_value(vm);
            let heap = &vm.bx.heap;
            let fill_obj = fill.as_object().expect("a named variant is an object");
            assert_eq!(
                enum_info(heap, fill_obj).map(|(e, v)| (live_id_token(e), live_id_token(v))),
                Some(("Size".to_string(), "Fill".to_string()))
            );
            assert_eq!(
                fmt_value(heap, fill, 2).as_deref(),
                Some("Size.Fill{weight: 100 basis: FitBound.Abs(0) shrink: 0}")
            );
            assert_eq!(fmt_value(heap, fixed, 2).as_deref(), Some("Size.Fixed(200)"));
            assert_eq!(fmt_value(heap, down, 2).as_deref(), Some("Flow.Down"));
            // Display drops the enum and the fields equal to the defaults.
            assert_eq!(fmt_enum(heap, fill_obj, EnumFmt::Display).as_deref(), Some("Fill"));
        });
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { pos: dvec2(x, y), size: dvec2(w, h) }
    }

    #[test]
    fn a_mention_lands_after_the_at_that_armed_it() {
        assert_eq!(insert_mention("look at @", "/a/b/c"), "look at @/a/b/c");
        // Typed on mid-sentence: the name goes where the @ is, not at the end.
        assert_eq!(insert_mention("@ is too wide", "./b"), "@./b is too wide");
        // The last @ wins — an earlier mention is left alone.
        assert_eq!(insert_mention("@/x/y and @", "../w"), "@/x/y and @../w");
        // The arming @ was deleted mid-pick: append rather than lose the click.
        assert_eq!(insert_mention("align these", "/a/b"), "align these @/a/b");
        assert_eq!(insert_mention("", "/a/b"), "@/a/b");
    }

    #[test]
    fn a_path_segment_says_what_the_widget_is() {
        // A real name wins.
        assert_eq!(readable_segment("main_window", "Window"), "main_window");
        // No name: the type is what a person would call it.
        assert_eq!(readable_segment("-", "View"), "View");
        // A list item's index is not a name either — the type reads better.
        assert_eq!(readable_segment("3", "Label"), "Label");
        // Neither: something has to be said.
        assert_eq!(readable_segment("-", "-"), "Widget");
    }

    #[test]
    fn an_anonymous_segment_is_a_position_not_a_name() {
        // Legacy `-` / `-2` stand-ins can still arrive from a hand-written
        // path or an older note store; the loose finder must drop them
        // rather than hash them into ids.
        assert!(is_anonymous_segment("-"));
        assert!(is_anonymous_segment("-0"));
        assert!(is_anonymous_segment("-12"));
        assert!(!is_anonymous_segment("-a"));
        assert!(!is_anonymous_segment("main_window"));
        // `Label.2` is a NAME with an ordinal, not a position stand-in: the
        // slash is the hierarchy, the dot is only which one of several.
        assert!(!is_anonymous_segment("Label.2"));
    }

    #[test]
    fn mentions_are_read_back_out_of_the_note_text() {
        let base = "/root/a/b";
        assert_eq!(
            note_mentions(base, "match @/a/b/c to @/d/e, please"),
            vec!["/a/b/c".to_string(), "/d/e".to_string()]
        );
        // A sentence-ending dot is punctuation, not part of the reference.
        assert_eq!(note_mentions(base, "like @/a/b."), vec!["/a/b".to_string()]);
        // The same widget twice is one reference.
        assert_eq!(
            note_mentions(base, "@/a/b and @/a/b"),
            vec!["/a/b".to_string()]
        );
        // A bare @ (armed, never picked) names nothing.
        assert!(note_mentions(base, "waiting on @").is_empty());
        assert!(note_mentions(base, "no mentions here").is_empty());
        // What the card SHOWS is relative; what comes back out is absolute,
        // because a reference read elsewhere has no "here" to be relative to.
        assert_eq!(
            note_mentions(base, "inside @./x"),
            vec!["/root/a/b/x".to_string()]
        );
        assert_eq!(
            note_mentions(base, "next to @../c"),
            vec!["/root/a/c".to_string()]
        );
    }

    #[test]
    fn a_relative_mention_reads_like_a_url() {
        // One notation for the whole scheme: `/` is the hierarchy, `./` the
        // noted widget, `../` its parent.
        let base = "/dock/tab/View.1/View.2/Label.1";
        // Inside the noted widget.
        assert_eq!(
            relative_path(base, "/dock/tab/View.1/View.2/Label.1/child").as_deref(),
            Some("./child")
        );
        // A sibling: up to the parent, then down.
        assert_eq!(
            relative_path(base, "/dock/tab/View.1/View.2/Label.3").as_deref(),
            Some("../Label.3")
        );
        // One level further out.
        assert_eq!(
            relative_path(base, "/dock/tab/View.1/Button").as_deref(),
            Some("../../Button")
        );
        // An ancestor gets no relative form: `../` alone names it but says
        // nothing about WHAT it is.
        assert!(relative_path(base, "/dock/tab/View.1/View.2").is_none());
        // Nothing in common: there is no honest relative form.
        assert!(relative_path("/a/b", "/x/y").is_none());
    }

    #[test]
    fn a_relative_mention_round_trips_to_the_same_widget() {
        let base = "/dock/tab/View.1/View.2/Label.1";
        for target in [
            "/dock/tab/View.1/View.2/Label.1/child",
            "/dock/tab/View.1/View.2/Label.3",
            "/dock/tab/View.1/Button",
            "/dock/other/Label.1",
        ] {
            let rel = relative_path(base, target).expect("shares a root");
            assert_eq!(absolute_mention(base, &rel), target, "{rel} did not round-trip");
        }
        // An absolute reference is left as it is, leading slash or not.
        assert_eq!(absolute_mention("/a/b", "/x/y/z"), "/x/y/z");
        assert_eq!(absolute_mention("/a/b", "x/y/z"), "/x/y/z");
    }

    #[test]
    fn the_store_reads_both_its_own_shape_and_the_one_before_it() {
        // The current shape: path, notes, rules.
        let now = note_store_parse("# header
Root/Button_1	too tight	keep it 44 high
");
        assert_eq!(now.len(), 1);
        assert_eq!(now[0].path, "Root/Button_1");
        assert_eq!(now[0].text, "too tight");
        assert_eq!(now[0].rules, "keep it 44 high");

        // A file written before rules existed: two columns, no third.
        let two = note_store_parse("Root/Label_2	just a note
");
        assert_eq!(two.len(), 1);
        assert_eq!(two[0].text, "just a note");
        assert!(two[0].rules.is_empty());

        // The shape the note CARD wrote: four columns of geometry it no
        // longer has anything to describe, with the text in column five.
        // The text has to survive; the geometry is dropped.
        let old = note_store_parse("Root/View_3	8	-108	230	96	the old card said this
");
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].path, "Root/View_3");
        assert_eq!(old[0].text, "the old card said this");
        assert!(old[0].rules.is_empty());

        // Newlines survive the round trip through a one-line-per-note file.
        let round = note_store_parse(&format!(
            "p	{}	{}
",
            note_store_escape("line one
line two"),
            note_store_escape("rule	with a tab")
        ));
        assert_eq!(round[0].text, "line one
line two");
        assert_eq!(round[0].rules, "rule	with a tab");
    }

    #[test]
    fn note_store_escapes_survive_a_round_trip() {
        // The store is one tab-separated line per note, so a note carrying
        // newlines, tabs or backslashes has to come back exactly as typed.
        for text in [
            "tighten the line spacing",
            "line one\nline two\n\nline four",
            "a\tb\\c\\nnot-a-newline",
            "",
        ] {
            let round = note_store_unescape(&note_store_escape(text));
            assert_eq!(round, text, "{text:?} did not survive");
        }
        // Nothing escaped may carry the separators themselves.
        let escaped = note_store_escape("a\tb\nc");
        assert!(!escaped.contains('\t') && !escaped.contains('\n'), "{escaped:?}");
    }

    #[test]
    fn a_malformed_note_line_is_skipped_not_fatal() {
        // The file is meant to be hand-editable, so short or commented lines
        // are dropped rather than taken as a note.
        let body = "# header\n\nnot-enough-columns\tx\n";
        let notes: Vec<&str> = body
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .filter(|line| line.split('\t').count() >= 6)
            .collect();
        assert!(notes.is_empty());
    }

    fn entry(seq: u64, path: &str, prop: &str, old: &str, new: &str) -> TweakDiffEntry {
        TweakDiffEntry {
            origin: String::new(),
            siblings: 0,
            scope: "this".to_string(),
            seq,
            path: path.to_string(),
            prop: prop.to_string(),
            old: old.to_string(),
            new: new.to_string(),
        }
    }

    #[test]
    fn diff_coalescing_keeps_first_old_and_last_new() {
        let entries = vec![
            entry(1, "a.b", "padding.left", "4", "10"),
            entry(2, "a.b", "padding.left", "10", "16"),
            entry(3, "a.c", "color", "#ff0000ff", "#00ff00ff"),
            entry(4, "a.b", "padding.left", "16", "24"),
        ];
        let coalesced = coalesce_diff(&entries);
        assert_eq!(coalesced.len(), 2);
        assert_eq!(coalesced[0].path, "a.b");
        assert_eq!(coalesced[0].old, "4");
        assert_eq!(coalesced[0].new, "24");
        assert_eq!(coalesced[1].path, "a.c");
    }

    #[test]
    fn diff_coalescing_drops_churn_back_to_original() {
        let entries = vec![
            entry(1, "a.b", "height", "40", "60"),
            entry(2, "a.b", "height", "60", "40"),
            entry(3, "a.b", "width", "10", "20"),
        ];
        let coalesced = coalesce_diff(&entries);
        assert_eq!(coalesced.len(), 1);
        assert_eq!(coalesced[0].prop, "width");
    }

    #[test]
    fn json_strings_escape() {
        assert_eq!(json_str("a\"b\n"), "\"a\\\"b\\n\"");
    }

    #[test]
    fn single_prop_chunks_parse() {
        assert_eq!(
            single_prop_chunk("padding.left: 12"),
            Some(("padding.left".to_string(), "12".to_string()))
        );
        assert_eq!(
            single_prop_chunk("{draw_bg.border_radius: 8}"),
            Some(("draw_bg.border_radius".to_string(), "8".to_string()))
        );
        // multi-property or nested chunks are not single props
        assert_eq!(single_prop_chunk("{a: 1, b: 2}"), None);
        assert_eq!(single_prop_chunk("padding: Inset{left: 4}"), None);
        assert_eq!(single_prop_chunk("draw_bg +: {color: #f00}"), None);
    }

    #[test]
    fn fmt_f64_stays_compact() {
        assert_eq!(fmt_f64(12.0), "12");
        assert_eq!(fmt_f64(2.5), "2.5");
        assert_eq!(fmt_f64(0.33333333), "0.3333");
    }
}
