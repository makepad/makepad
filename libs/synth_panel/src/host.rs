//! What the synth widgets need beyond paths, from the desk that mounts a
//! score's editor panel: what a node is, its live sound, the alternatives
//! it could be switched to, a key to hear it on, and where a person's
//! attention went (a click on a display or a key: the selection the
//! score's agent is told).
//!
//! Values are read and written through the panel widgets' host
//! (`makepad_value_pickers::panel::host`), so a synth widget's drag is
//! held live and written once like any panel control's.

use crate::live::LiveSource;
use makepad_score_live::instruments::Choice;
use makepad_widgets::WidgetUid;
use std::cell::{Cell, RefCell};

/// A voice (a loop of the score) as the voice tree shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub name: String,
    /// The instrument it plays (its node), and that in words.
    pub instrument: Option<String>,
    pub inst_label: String,
    pub amp: f64,
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
    /// One cycle of what it plays: (beat, note, dur, velocity, hit).
    pub notes: Vec<(f64, f32, f32, f32, bool)>,
    pub beats: f64,
    pub beats_per_bar: f64,
    /// The arrangement: (start beat, beats, whether it plays there).
    pub sections: Vec<(f64, f64, bool)>,
}

/// A card of the melody variants: its label, its notes (as a track's),
/// whether it is kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub label: String,
    pub notes: Vec<(f64, f32, f32, f32, bool)>,
    pub kept: bool,
}

/// The variants of a voice: the original first, then the cards; what is
/// being made, and which card plays (0 the original).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variants {
    /// The part's length in beats (its phrase: the cycles it walks through
    /// before they repeat); 0 before the part was read.
    pub beats: f64,
    pub cards: Vec<Card>,
    pub busy: Option<String>,
    pub playing: Option<usize>,
    /// The region the next variants change (beats), if any.
    pub region: Option<(f64, f64)>,
}

/// What a person does with the variants.
#[derive(Clone, Debug, PartialEq)]
pub enum VaryAction {
    /// Make variants: a kind (`variations`, `syncopated` …) or, with `ai`,
    /// an ask in words; `n` of them.
    Make { kind: String, ai: bool, n: usize },
    /// Change only these beats from now on (None: the whole part).
    Region(Option<(f64, f64)>),
    /// Hear a card looped in place (0: the original; None: stop).
    Audition(Option<usize>),
    Keep(usize),
    /// Make the voice play a card (one undo step).
    Apply(usize),
    /// Bar by bar from the cards chosen (index per bar), one undo step.
    Merge(Vec<usize>),
    /// Drop the cards not kept.
    Clear,
}
use std::sync::Arc;

pub struct SynthHost {
    /// A node's type (`Synth`, `Shader`, `Drums`, `Instrument`, an FX, `Loop`).
    pub type_of: Box<dyn Fn(&str) -> Option<String>>,
    /// The playing score's meters and scopes.
    pub live: Option<Arc<dyn LiveSource>>,
    /// What `node` could become, grouped as [`makepad_score_live::instruments::GROUPS`],
    /// and the key of what it is now.
    pub choices: Box<dyn Fn(&str) -> (Vec<Choice>, Option<String>)>,
    /// Hear `node` as `choice` (in place, a short phrase); `None` stops.
    pub preview: Box<dyn Fn(&str, Option<&Choice>)>,
    /// Make `node` that instrument (one undo step).
    pub switch: Box<dyn Fn(&str, &Choice)>,
    /// A key on `node` pressed (`true`) or let go.
    pub key: Box<dyn Fn(&str, f32, bool)>,
    /// A person pointed at `node` (a display, a key).
    pub select: Box<dyn Fn(&str)>,
    /// Generator `gen` on loop `node` with these choices: heard (`false`)
    /// or written (`true`, one undo step).
    pub generate: Box<dyn Fn(&str, &str, &[(String, String)], bool)>,
    /// The score's voices, in its order.
    pub tracks: Box<dyn Fn() -> Vec<Track>>,
    /// A voice's melody variants, and what is done with them.
    pub variants: Box<dyn Fn(&str) -> Variants>,
    pub vary: Box<dyn Fn(&str, VaryAction)>,
}

thread_local! {
    static HOST: RefCell<Option<SynthHost>> = const { RefCell::new(None) };
    /// The node selected in the panel (a voice or an instrument): what the
    /// details under the tree show.
    static SELECTED: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Counts the selections (a click on the same node again counts too).
    static SELECTIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The last selection was for the voice's variations (its instrument's
    /// controls stay as they are).
    static FOR_VARIATIONS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The widgets the voice tree draws inline under a voice, by uid.
    static INLINE: RefCell<Vec<(Inline, WidgetUid)>> = const { RefCell::new(Vec::new()) };
    /// The tree is drawing one of them now (in its place under the voice).
    static DRAWING_INLINE: Cell<bool> = const { Cell::new(false) };
    /// An instrument search has the keys (a space types, it does not play).
    static SEARCHING: Cell<bool> = const { Cell::new(false) };
}

/// What the voice tree unfolds under a voice: its melody variants, or the
/// tweaks written for a node (its instrument, or the voice itself).
#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Track,
    Tweaks(String),
}

/// A `SynthTrack` or `SynthTweaks` of the panel, for the tree to draw in
/// its place under the voice it belongs to.
pub fn register_inline(what: Inline, uid: WidgetUid) {
    INLINE.with(|list| {
        let mut list = list.borrow_mut();
        match list.iter_mut().find(|(_, u)| *u == uid) {
            Some(entry) => entry.0 = what,
            None => list.push((what, uid)),
        }
    });
}

/// The widgets registered for the tree to draw inline.
pub fn inline_widgets() -> Vec<(Inline, WidgetUid)> {
    INLINE.with(|list| list.borrow().clone())
}

/// Forget a registered widget (it is gone with the panel it was in).
pub fn forget_inline(uid: WidgetUid) {
    INLINE.with(|list| list.borrow_mut().retain(|(_, u)| *u != uid));
}

/// Whether the tree is drawing an inline widget now (else it is its
/// parent's turn, where it takes no room).
pub fn drawing_inline() -> bool {
    DRAWING_INLINE.with(|d| d.get())
}

pub fn set_drawing_inline(on: bool) {
    DRAWING_INLINE.with(|d| d.set(on));
}

/// An instrument search has the keys: the page's space plays nothing.
pub fn searching() -> bool {
    SEARCHING.with(|s| s.get())
}

pub fn set_searching(on: bool) {
    SEARCHING.with(|s| s.set(on));
}

/// The last selection was for the variations, not the controls.
pub fn selected_for_variations() -> bool {
    FOR_VARIATIONS.with(|f| f.get())
}

/// A voice selected for its variations: its timeline and cards show under
/// the tree; its instrument's controls are left as they are.
pub fn select_variations(node: &str) {
    select(node);
    FOR_VARIATIONS.with(|f| f.set(true));
}

/// Rises with every selection: what shows for it opens and comes into view.
pub fn selection_rev() -> u64 {
    SELECTIONS.with(|s| s.get())
}

/// The node selected in the panel.
pub fn selected() -> Option<String> {
    SELECTED.with(|s| s.borrow().clone())
}

pub fn install(host: SynthHost) {
    HOST.with(|h| *h.borrow_mut() = Some(host));
}

pub fn uninstall() {
    HOST.with(|h| *h.borrow_mut() = None);
}

fn with<R>(f: impl FnOnce(&SynthHost) -> R) -> Option<R> {
    HOST.with(|h| h.borrow().as_ref().map(f))
}

pub fn type_of(node: &str) -> Option<String> {
    with(|h| (h.type_of)(node)).flatten()
}

pub fn live() -> Option<Arc<dyn LiveSource>> {
    with(|h| h.live.clone()).flatten()
}

pub fn choices(node: &str) -> (Vec<Choice>, Option<String>) {
    with(|h| (h.choices)(node)).unwrap_or_default()
}

pub fn preview(node: &str, choice: Option<&Choice>) {
    with(|h| (h.preview)(node, choice));
}

pub fn switch(node: &str, choice: &Choice) {
    with(|h| (h.switch)(node, choice));
}

pub fn key(node: &str, note: f32, down: bool) {
    with(|h| (h.key)(node, note, down));
}

/// A person pointed at `node`: it is selected (the details follow) and
/// the desk is told.
pub fn select(node: &str) {
    SELECTED.with(|s| *s.borrow_mut() = Some(node.to_string()));
    SELECTIONS.with(|s| s.set(s.get() + 1));
    FOR_VARIATIONS.with(|f| f.set(false));
    with(|h| (h.select)(node));
}

/// The panel's selection cleared (everything but the tree folds).
pub fn unselect() {
    SELECTED.with(|s| *s.borrow_mut() = None);
}

pub fn tracks() -> Vec<Track> {
    with(|h| (h.tracks)()).unwrap_or_default()
}

pub fn variants(node: &str) -> Variants {
    with(|h| (h.variants)(node)).unwrap_or_default()
}

pub fn vary(node: &str, action: VaryAction) {
    with(|h| (h.vary)(node, action));
}

pub fn generate(node: &str, gen: &str, choices: &[(String, String)], write: bool) {
    with(|h| (h.generate)(node, gen, choices, write));
}
