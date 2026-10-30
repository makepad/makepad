//! The instrument browser: one panel for every place an instrument is
//! picked (a lens card's chip in the code view, the tracker's track
//! settings, the editor panel), anchored to what opened it or popped out
//! into its own small window that stays on top.
//!
//! - Left: where to look: favourites, recent, the instruments this score
//!   uses, then the engines (each a category of its own: Synth, Morph,
//!   Drums, General MIDI, Piano …, and any engine added later) and the
//!   categories (bass, lead, pad, keys …), each with its count.
//! - Middle: the presets that match, a row each with its picture (an
//!   envelope over its wave, a wavetable's frame, a kit's voices), drawn
//!   only where they show; character tags (bright, dark, mono …) narrow it.
//! - Right: the one under the selection: what it is, its settings, a kit's
//!   pads to play, and "similar".
//!
//! It stays open. A click (or the arrow keys) makes the node that preset at
//! once, heard in the playing loop, one undo step per pick; resting on a
//! row auditions it without picking. A/B swaps the last two picks; Revert
//! puts back what the node was when the browser opened. Enter keeps and
//! closes, Esc closes (clearing the search first), and so does a click
//! outside an anchored browser. Typing searches name, tags and pack.
//!
//! Its data is the preset index (`makepad_score_live::browse`), built once
//! off the UI thread; until it is there the browser says so and waits.
//! It talks to the score through the synth widgets' host ([`crate::host`]).

use crate::host as synth;
use makepad_score_live::browse::{self, Entry, Facets, Index, Thumb};
use makepad_value_pickers::panel::control::{DrawPanelShape, R_CONTROL, R_GROUP};
use makepad_widgets::badge::advance;
use makepad_widgets::*;
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.BrowserCore = set_type_default() do #(Browser::script_component(vm)){
        draw_text +: {text_style: theme.font_regular{font_size: 8.5}}
        draw_bold +: {text_style: theme.font_bold{font_size: 9}}
        draw_title +: {text_style: theme.font_bold{font_size: 11}}
        draw_small +: {text_style: theme.font_regular{font_size: 7.5}}
        draw_caps +: {text_style: theme.font_bold{font_size: 6.5}}
    }

    mod.widgets.InstrumentBrowserBase = #(InstrumentBrowser::register_widget(vm))
    /** The instrument browser as a widget (a window's, a panel's). */
    mod.widgets.InstrumentBrowser = set_type_default() do mod.widgets.InstrumentBrowserBase{
        width: Fill height: Fill
        draw_bg +: {color: #x00000000}
    }
}

fn color(value: u32, alpha: f32) -> Vec4f {
    let mut c = Vec4f::from_u32(value);
    c.w *= alpha;
    c
}

const HEAD: f64 = 34.0;
const LEFT_W: f64 = 150.0;
const RIGHT_W: f64 = 230.0;
const ROW: f64 = 26.0;
const SIDE_ROW: f64 = 19.0;
/// Seconds a row is rested on before it is heard.
const REST: f64 = 0.18;
/// How many picks Recent keeps.
const RECENT: usize = 24;

/// Where the list looks.
#[derive(Clone, Debug, PartialEq)]
enum Source {
    Favourites,
    Recent,
    Used,
    /// The presets nearest one (by its key).
    Similar(String),
}

/// What the browser asks of its holder after an event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserOut {
    pub redraw: bool,
    /// Close it (Enter, Esc, the close button, a click outside).
    pub close: bool,
    /// Pop it out into a window (`Some(true)`) or back into its card.
    pub pop: Option<bool>,
    /// The event was the browser's.
    pub consumed: bool,
}

type Name = &'static str;

/// A control drawn this frame.
#[derive(Clone, Debug)]
enum Hit {
    Row(usize),
    Star(usize),
    Source(Source),
    Engine(Option<Name>),
    Category(Option<Name>),
    Tag(String),
    Pack(Option<String>),
    Ab,
    Revert,
    Pop,
    Close,
    Clear,
    Similar(usize),
    Pad(usize, Name),
    Audition(usize),
    Library(String),
}

/// The preset index once built (a type of its own for the derive).
type IndexRef = Option<&'static Index>;

#[derive(Script, ScriptHook)]
pub struct Browser {
    #[live]
    draw_shape: DrawPanelShape,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_bold: DrawText,
    #[live]
    draw_title: DrawText,
    #[live]
    draw_small: DrawText,
    #[live]
    draw_caps: DrawText,
    /// The node it picks for.
    #[rust]
    pub node: String,
    #[rust]
    open: bool,
    /// Popped out into its own window.
    #[rust]
    pub popped: bool,
    /// Part of a panel (the editor panel's voice tree): a click outside
    /// leaves it open, and it draws flat.
    #[rust]
    pub inline: bool,
    #[rust]
    index: IndexRef,
    /// The score's own entries (its instruments, SoundFonts, samples),
    /// before the index's.
    #[rust]
    extra: Vec<Entry>,
    /// Entry by key (extra first).
    #[rust]
    by_key: HashMap<String, usize>,
    #[rust]
    current: Option<String>,
    /// The node's constructor when it opened, and before the last pick.
    #[rust]
    opened: Option<String>,
    #[rust]
    before: Option<String>,
    #[rust]
    query: String,
    #[rust]
    source: Option<Source>,
    #[rust]
    facets: Facets,
    /// The entries shown (indices into extra ++ index).
    #[rust]
    view: Vec<usize>,
    #[rust]
    dirty: bool,
    #[rust]
    sel: usize,
    #[rust]
    scroll: f64,
    /// Keep the selection in view on the next draw.
    #[rust]
    reveal: bool,
    #[rust]
    favourites: Vec<String>,
    #[rust]
    recent: Vec<String>,
    #[rust]
    used: Vec<String>,
    /// The row under the pointer and since when; the one heard.
    #[rust]
    hover: Option<(usize, f64)>,
    #[rust]
    heard: Option<usize>,
    #[rust]
    mouse: Option<DVec2>,
    #[rust]
    hits: Vec<(Rect, Hit)>,
    #[rust]
    rect: Rect,
    #[rust]
    list: Rect,
    /// Counts per engine and category (of what the search and the other
    /// facet leave), made with the view.
    #[rust]
    engines: Vec<(Name, usize)>,
    #[rust]
    categories: Vec<(Name, usize)>,
    #[rust]
    tags: Vec<(String, usize)>,
    #[rust]
    packs: Vec<(String, usize)>,
    /// The left column's scroll, and its rect.
    #[rust]
    left_scroll: f64,
    #[rust]
    left_rect: Rect,
    /// The engine facet was set by opening (on the node's engine), not by
    /// a person: typing a search lifts it.
    #[rust]
    auto_engine: bool,
}

impl Browser {
    /// Draw over what was drawn before it (an anchored browser over the
    /// code and its cards).
    pub fn layer(&mut self, cx: &mut Cx2d) {
        self.draw_shape.new_draw_call(cx);
        self.draw_text.new_draw_call(cx);
        self.draw_bold.new_draw_call(cx);
        self.draw_title.new_draw_call(cx);
        self.draw_small.new_draw_call(cx);
        self.draw_caps.new_draw_call(cx);
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open it on `node` (what it is now marked, its search empty).
    pub fn open(&mut self, node: &str) {
        let same = self.node == node && self.open;
        self.node = node.to_string();
        self.open = true;
        if !same {
            self.opened = synth::ctor(node);
            self.before = None;
            self.query.clear();
            self.source = None;
            self.facets = Facets::default();
            self.scroll = 0.0;
        }
        self.favourites = synth::recall("browser.favourites").map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default();
        self.recent = synth::recall("browser.recent").map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default();
        self.refresh();
        // Start where it is: its engine, its row selected.
        if !same {
            if let Some(at) = self.current.as_ref().and_then(|k| self.by_key.get(k)).copied() {
                self.facets.engine = self.entry(at).map(|e| e.engine.to_string());
                self.auto_engine = true;
                self.dirty = true;
                self.rebuild();
                self.sel = self.view.iter().position(|i| *i == at).unwrap_or(0);
                self.reveal = true;
            }
        }
    }

    pub fn close(&mut self) {
        if self.open {
            self.stop_preview();
        }
        self.open = false;
    }

    /// The score changed: what the node is now, the score's own entries.
    pub fn refresh(&mut self) {
        self.index = browse::index();
        self.extra = synth::extras();
        self.current = synth::current(&self.node);
        self.used = synth::used();
        self.by_key.clear();
        let n = self.extra.len();
        for (i, e) in self.extra.iter().enumerate() {
            self.by_key.entry(e.choice.key()).or_insert(i);
        }
        if let Some(index) = self.index {
            for (i, e) in index.entries.iter().enumerate() {
                self.by_key.entry(e.choice.key()).or_insert(n + i);
            }
        }
        self.dirty = true;
    }

    /// The node it is bound to was renamed.
    pub fn rebind(&mut self, node: &str) {
        self.node = node.to_string();
        self.current = synth::current(node);
    }

    fn entry(&self, i: usize) -> Option<&Entry> {
        let n = self.extra.len();
        if i < n {
            self.extra.get(i)
        } else {
            self.index.and_then(|x| x.entries.get(i - n))
        }
    }

    fn total(&self) -> usize {
        self.extra.len() + self.index.map_or(0, |x| x.entries.len())
    }

    /// The view again (after the search, a facet or the source changed).
    fn rebuild(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let all: Vec<usize> = match &self.source {
            None => (0..self.total()).collect(),
            Some(Source::Favourites) => self.favourites.iter().filter_map(|k| self.by_key.get(k).copied()).collect(),
            Some(Source::Recent) => self.recent.iter().filter_map(|k| self.by_key.get(k).copied()).collect(),
            Some(Source::Used) => self.used.iter().filter_map(|k| self.by_key.get(k).copied()).collect(),
            Some(Source::Similar(key)) => match self.by_key.get(key).copied() {
                Some(of) => {
                    let n = self.extra.len();
                    match self.index {
                        Some(index) if of >= n => std::iter::once(of).chain(index.similar(of - n, 40).into_iter().map(|i| n + i)).collect(),
                        _ => vec![of],
                    }
                }
                None => Vec::new(),
            },
        };
        let words: Vec<String> = self.query.to_lowercase().replace('_', " ").split_whitespace().map(str::to_string).collect();
        let mut engines: Vec<(&'static str, usize)> = Vec::new();
        let mut categories: Vec<(&'static str, usize)> = Vec::new();
        let mut tags: HashMap<String, usize> = HashMap::new();
        let mut packs: Vec<(String, usize)> = Vec::new();
        let mut view = Vec::new();
        for i in all {
            let Some(e) = self.entry(i) else { continue };
            if !e.matches(&words) || !self.facets.tags.iter().all(|t| e.tags.iter().any(|x| x == t)) {
                continue;
            }
            let pack_ok = self.facets.pack.as_deref().is_none_or(|p| e.pack == p);
            match packs.iter_mut().find(|(n, _)| *n == e.pack) {
                Some((_, c)) => *c += 1,
                None => packs.push((e.pack.clone(), 1)),
            }
            if !pack_ok {
                continue;
            }
            let engine_ok = self.facets.engine.as_deref().is_none_or(|x| e.engine == x);
            let cat_ok = self.facets.category.as_deref().is_none_or(|x| e.category == x);
            if cat_ok {
                match engines.iter_mut().find(|(n, _)| *n == e.engine) {
                    Some((_, c)) => *c += 1,
                    None => engines.push((e.engine, 1)),
                }
            }
            if engine_ok {
                match categories.iter_mut().find(|(n, _)| *n == e.category) {
                    Some((_, c)) => *c += 1,
                    None => categories.push((e.category, 1)),
                }
            }
            if engine_ok && cat_ok {
                for t in &e.tags {
                    *tags.entry(t.clone()).or_default() += 1;
                }
                view.push(i);
            }
        }
        categories.sort_by_key(|(c, _)| browse::CATEGORIES.iter().position(|x| x == c).unwrap_or(99));
        let mut tags: Vec<(String, usize)> = tags.into_iter().filter(|(_, n)| *n > 1 && *n < view.len()).collect();
        // Character first, then the commonest.
        let character = ["bright", "dark", "warm", "mono", "poly"];
        tags.sort_by(|a, b| {
            let ca = character.iter().position(|c| *c == a.0).unwrap_or(9);
            let cb = character.iter().position(|c| *c == b.0).unwrap_or(9);
            ca.cmp(&cb).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0))
        });
        tags.truncate(10);
        for t in &self.facets.tags {
            if !tags.iter().any(|(x, _)| x == t) {
                tags.insert(0, (t.clone(), 0));
            }
        }
        // A search ranks what it names first: the name itself, then names
        // that start with it or hold it, then the rest (in index order).
        if !words.is_empty() {
            let rank = |e: &Entry| {
                let name = e.choice.label.to_lowercase();
                let q = words.join(" ");
                if name == q {
                    0
                } else if name.starts_with(&q) {
                    1
                } else if words.iter().all(|w| name.contains(w.as_str())) {
                    2
                } else if words.iter().all(|w| e.tags.iter().any(|t| t.contains(w.as_str())) || e.category.contains(w.as_str())) {
                    3
                } else {
                    4
                }
            };
            let mut ranked: Vec<(u8, usize)> = view.iter().map(|i| (self.entry(*i).map_or(9, |e| rank(e)), *i)).collect();
            ranked.sort_by_key(|(r, _)| *r);
            view = ranked.into_iter().map(|(_, i)| i).collect();
        }
        self.engines = engines;
        self.categories = categories;
        self.packs = packs;
        self.tags = tags;
        self.view = view;
        self.sel = self.sel.min(self.view.len().saturating_sub(1));
    }

    fn set_dirty(&mut self) {
        self.dirty = true;
        self.sel = 0;
        self.scroll = 0.0;
    }

    /// Make the node the entry `i` (one undo step), remembered as recent.
    fn pick(&mut self, i: usize) {
        let Some(e) = self.entry(i).cloned() else { return };
        let key = e.choice.key();
        if self.current.as_deref() == Some(key.as_str()) {
            return;
        }
        self.stop_preview();
        self.before = synth::ctor(&self.node);
        synth::switch(&self.node, &e.choice);
        self.current = Some(key.clone());
        self.recent.retain(|k| *k != key);
        self.recent.insert(0, key);
        self.recent.truncate(RECENT);
        synth::remember("browser.recent", &self.recent.join("\n"));
    }

    /// A/B: the pick before the last, and back.
    fn swap(&mut self) {
        let Some(before) = self.before.take() else { return };
        self.stop_preview();
        self.before = synth::ctor(&self.node);
        synth::set_ctor(&self.node, &before);
        self.current = None;
    }

    /// Put back what the node was when the browser opened.
    fn revert(&mut self) {
        let Some(opened) = self.opened.clone() else { return };
        if synth::ctor(&self.node).as_deref() == Some(opened.as_str()) {
            return;
        }
        self.stop_preview();
        self.before = synth::ctor(&self.node);
        synth::set_ctor(&self.node, &opened);
        self.current = None;
    }

    fn star(&mut self, i: usize) {
        let Some(key) = self.entry(i).map(|e| e.choice.key()) else { return };
        if let Some(at) = self.favourites.iter().position(|k| *k == key) {
            self.favourites.remove(at);
        } else {
            self.favourites.insert(0, key);
        }
        synth::remember("browser.favourites", &self.favourites.join("\n"));
        if self.source == Some(Source::Favourites) {
            self.dirty = true;
        }
    }

    fn stop_preview(&mut self) {
        if self.heard.take().is_some() {
            synth::preview(&self.node, None);
        }
        self.hover = None;
    }

    fn move_sel(&mut self, to: usize) {
        if self.view.is_empty() {
            return;
        }
        self.sel = to.min(self.view.len() - 1);
        self.reveal = true;
        let i = self.view[self.sel];
        self.pick(i);
    }

    /// Resting on a row for a moment auditions it; true while it waits.
    pub fn tick(&mut self) -> bool {
        if self.index.is_none() {
            if browse::index().is_some() {
                self.refresh();
                return false;
            }
            return true;
        }
        if let Some((i, since)) = self.hover {
            if self.heard != Some(i) {
                if Cx::time_now() - since >= REST {
                    if let Some(e) = self.entry(i).cloned() {
                        if self.current.as_deref() != Some(e.choice.key().as_str()) {
                            synth::preview(&self.node, Some(&e.choice));
                            self.heard = Some(i);
                        }
                    }
                    return false;
                }
                return true;
            }
        }
        false
    }

    /// The browser's events (only while open). `inside` tells a click
    /// outside an anchored browser apart from one on it.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> BrowserOut {
        let mut out = BrowserOut::default();
        if !self.open {
            return out;
        }
        match event {
            Event::KeyDown(k) => {
                out.consumed = true;
                out.redraw = true;
                let n = self.view.len();
                match k.key_code {
                    KeyCode::Escape if !self.query.is_empty() => {
                        self.query.clear();
                        self.set_dirty();
                    }
                    KeyCode::Escape => {
                        self.close();
                        out.close = true;
                    }
                    KeyCode::ReturnKey => {
                        if let Some(i) = self.view.get(self.sel).copied() {
                            self.pick(i);
                        }
                        self.close();
                        out.close = true;
                    }
                    KeyCode::ArrowDown => self.move_sel(self.sel + 1),
                    KeyCode::ArrowUp => self.move_sel(self.sel.saturating_sub(1)),
                    KeyCode::PageDown => self.move_sel(self.sel + 10),
                    KeyCode::PageUp => self.move_sel(self.sel.saturating_sub(10)),
                    KeyCode::Home if n > 0 => self.move_sel(0),
                    KeyCode::End if n > 0 => self.move_sel(n - 1),
                    KeyCode::Backspace => {
                        self.query.pop();
                        self.set_dirty();
                    }
                    _ => {}
                }
            }
            Event::TextInput(t) => {
                out.consumed = true;
                out.redraw = true;
                if std::mem::take(&mut self.auto_engine) {
                    self.facets.engine = None;
                }
                self.query.push_str(&t.input);
                self.set_dirty();
            }
            Event::KeyUp(_) => out.consumed = true,
            Event::MouseDown(e) => {
                if !self.rect.contains(e.abs) {
                    if !self.popped && !self.inline {
                        self.close();
                        out.close = true;
                        out.redraw = true;
                    }
                    return out;
                }
                out.consumed = true;
                out.redraw = true;
                let hit = self.hits.iter().rev().find(|(r, _)| r.contains(e.abs)).map(|(_, h)| h.clone());
                match hit {
                    Some(Hit::Row(at)) => {
                        self.sel = at;
                        if let Some(i) = self.view.get(at).copied() {
                            self.pick(i);
                        }
                    }
                    Some(Hit::Star(i)) => self.star(i),
                    Some(Hit::Source(s)) => {
                        self.source = if self.source.as_ref() == Some(&s) { None } else { Some(s) };
                        self.set_dirty();
                    }
                    Some(Hit::Engine(e)) => {
                        self.auto_engine = false;
                        self.facets.engine = if self.facets.engine.as_deref() == e { None } else { e.map(str::to_string) };
                        self.facets.category = None;
                        self.set_dirty();
                    }
                    Some(Hit::Category(c)) => {
                        self.facets.category = if self.facets.category.as_deref() == c { None } else { c.map(str::to_string) };
                        self.set_dirty();
                    }
                    Some(Hit::Pack(pk)) => {
                        self.facets.pack = if self.facets.pack == pk { None } else { pk };
                        self.set_dirty();
                    }
                    Some(Hit::Tag(t)) => {
                        if let Some(at) = self.facets.tags.iter().position(|x| *x == t) {
                            self.facets.tags.remove(at);
                        } else {
                            self.facets.tags.push(t);
                        }
                        self.set_dirty();
                    }
                    Some(Hit::Ab) => self.swap(),
                    Some(Hit::Revert) => self.revert(),
                    Some(Hit::Pop) if self.inline => {
                        synth::pop_out(&self.node);
                        self.close();
                        out.close = true;
                    }
                    Some(Hit::Pop) => out.pop = Some(!self.popped),
                    Some(Hit::Close) => {
                        self.close();
                        out.close = true;
                    }
                    Some(Hit::Clear) => {
                        self.query.clear();
                        self.facets = Facets::default();
                        self.source = None;
                        self.set_dirty();
                    }
                    Some(Hit::Similar(i)) => {
                        if let Some(key) = self.entry(i).map(|e| e.choice.key()) {
                            self.source = Some(Source::Similar(key));
                            self.facets = Facets::default();
                            self.query.clear();
                            self.set_dirty();
                        }
                    }
                    Some(Hit::Pad(i, drum)) => {
                        if let Some(e) = self.entry(i).cloned() {
                            let current = self.current.as_deref() == Some(e.choice.key().as_str());
                            synth::pad(&self.node, if current { None } else { Some(&e.choice) }, drum);
                        }
                    }
                    Some(Hit::Library(engine)) => synth::get_library(&engine),
                    Some(Hit::Audition(i)) => {
                        if let Some(e) = self.entry(i).cloned() {
                            synth::preview(&self.node, Some(&e.choice));
                            self.heard = Some(i);
                        }
                    }
                    None => {}
                }
            }
            Event::MouseMove(e) => {
                self.mouse = Some(e.abs);
                if self.rect.contains(e.abs) {
                    out.consumed = true;
                    let over = self.hits.iter().rev().find(|(r, _)| r.contains(e.abs) && self.list.contains(e.abs)).and_then(|(_, h)| match h {
                        Hit::Row(at) => self.view.get(*at).copied(),
                        _ => None,
                    });
                    if over != self.hover.map(|(i, _)| i) {
                        if over.is_none() {
                            self.stop_preview();
                        } else {
                            self.hover = over.map(|i| (i, Cx::time_now()));
                        }
                    }
                    if self.hits.iter().any(|(r, _)| r.contains(e.abs)) {
                        cx.set_cursor(MouseCursor::Hand);
                    }
                    out.redraw = true;
                } else if self.hover.is_some() {
                    self.stop_preview();
                    out.redraw = true;
                }
            }
            Event::Scroll(e) if self.list.contains(e.abs) => {
                self.scroll += e.scroll.y;
                out.consumed = true;
                out.redraw = true;
            }
            Event::Scroll(e) if self.left_rect.contains(e.abs) => {
                self.left_scroll = (self.left_scroll + e.scroll.y).max(0.0);
                out.consumed = true;
                out.redraw = true;
            }
            Event::MouseUp(e) if self.rect.contains(e.abs) => out.consumed = true,
            _ => {}
        }
        out
    }

    /// Where an anchored browser goes for an anchor at `at` within `clip`.
    pub fn place(at: DVec2, clip: Rect) -> Rect {
        let w = (clip.size.x - 16.0).clamp(420.0, 900.0);
        let h = (clip.size.y - 12.0).clamp(240.0, 480.0);
        let x = at.x.min(clip.pos.x + clip.size.x - w - 8.0).max(clip.pos.x + 8.0);
        let below = at.y + h <= clip.pos.y + clip.size.y - 6.0;
        let y = if below { at.y } else { (at.y - h - 24.0).max(clip.pos.y + 6.0) };
        Rect { pos: dvec2(x, y), size: dvec2(w, h) }
    }

    fn hit(&mut self, r: Rect, h: Hit) {
        self.hits.push((r, h));
    }

    /// A small button with a label; lit while `on`.
    fn button(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette, at: DVec2, text: &str, on: bool, enabled: bool, hit: Hit) -> Rect {
        let w = advance(&self.draw_small, cx, text) + 14.0;
        let r = Rect { pos: at, size: dvec2(w, 20.0) };
        let under = self.mouse.is_some_and(|m| r.contains(m));
        let (fill, ink) = if on {
            (color(p.accent_wash, 1.0), color(p.accent, 1.0))
        } else if under && enabled {
            (color(p.control_hover, 1.0), color(p.text_hi, 1.0))
        } else {
            (color(p.control, 1.0), color(if enabled { p.label } else { p.dim }, 1.0))
        };
        self.draw_shape.edged_round(cx, r, fill, color(p.line, 1.0), 1.0, R_CONTROL);
        self.draw_small.color = ink;
        self.draw_small.draw_abs(cx, dvec2(r.pos.x + 7.0, r.pos.y + 4.0), text);
        if enabled {
            self.hit(r, hit);
        }
        r
    }

    /// Draw it into `rect`; true while something on it moves.
    pub fn draw(&mut self, cx: &mut Cx2d, rect: Rect) -> bool {
        if !self.open {
            return false;
        }
        if self.index.is_none() && browse::index().is_some() {
            self.refresh();
        }
        self.rebuild();
        let p = makepad_look::palette::get(cx);
        self.rect = rect;
        self.hits.clear();
        if !self.popped && !self.inline {
            // A shadow, then the panel.
            self.draw_shape.fill_round(cx, Rect { pos: rect.pos + dvec2(0.0, 3.0), size: rect.size }, color(0, 0.35), R_GROUP + 2.0);
        }
        self.draw_shape.edged_round(cx, rect, color(p.menu, 1.0), color(p.border_hi, 1.0), 1.0, if self.popped { 0.0 } else { R_GROUP + 1.0 });
        // The head: the node and what it is, the search, the session's
        // tools.
        let x0 = rect.pos.x + 12.0;
        let hy = rect.pos.y + 8.0;
        // Narrow (a panel's column, a small window): the search on a row of
        // its own.
        let narrow = rect.size.x < 520.0;
        self.draw_bold.color = color(p.text_bright, 1.0);
        self.draw_bold.draw_abs(cx, dvec2(x0, hy + 2.0), &self.node.clone());
        let mut x = x0 + advance(&self.draw_bold, cx, &self.node.clone()) + 12.0;
        let now = self.current.as_ref().and_then(|k| self.by_key.get(k)).and_then(|i| self.entry(*i)).map(|e| format!("{} · {}", e.engine, e.choice.label));
        if let (Some(now), false) = (now, narrow) {
            self.draw_small.color = color(p.secondary, 1.0);
            let (now, _) = makepad_value_pickers::panel::control::fit_text(&self.draw_small, cx, &now, 160.0);
            self.draw_small.draw_abs(cx, dvec2(x, hy + 4.0), &now);
            x += advance(&self.draw_small, cx, &now) + 12.0;
        }
        // The tools at the right.
        let mut rx = rect.pos.x + rect.size.x - 10.0;
        let pop = if self.popped { "Dock" } else { "Pop out" };
        let can_pop = self.popped || synth::can_pop_out();
        let tools: [(&str, bool, bool, Hit); 4] = [
            ("✕", false, true, Hit::Close),
            (if can_pop { pop } else { "" }, false, can_pop, Hit::Pop),
            ("Revert", false, self.opened.is_some(), Hit::Revert),
            ("A/B", false, self.before.is_some(), Hit::Ab),
        ];
        for (text, on, enabled, hit) in tools {
            if text.is_empty() {
                continue;
            }
            let w = advance(&self.draw_small, cx, text) + 14.0;
            rx -= w;
            self.button(cx, &p, dvec2(rx, hy), text, on, enabled, hit);
            rx -= 4.0;
        }
        // The search field: beside the name, or its own row when narrow.
        let search = if narrow {
            Rect { pos: dvec2(x0, hy + 26.0), size: dvec2(rect.size.x - 24.0, 20.0) }
        } else {
            Rect { pos: dvec2(x, hy), size: dvec2((rx - 8.0 - x).max(80.0), 20.0) }
        };
        let head = if narrow { HEAD + 26.0 } else { HEAD };
        self.draw_shape.edged_round(cx, search, color(p.well, 1.0), color(if self.query.is_empty() { p.line } else { p.accent }, 1.0), 1.0, R_CONTROL);
        let (text, ink) = if self.query.is_empty() { ("type to search names, tags and packs".to_string(), p.dim) } else { (self.query.clone(), p.text_bright) };
        self.draw_text.color = color(ink, 1.0);
        let (shown, _) = makepad_value_pickers::panel::control::fit_text(&self.draw_text, cx, &text, search.size.x - 18.0);
        self.draw_text.draw_abs(cx, dvec2(search.pos.x + 8.0, search.pos.y + 3.5), &shown);
        if !self.query.is_empty() {
            let tw = advance(&self.draw_text, cx, &shown);
            self.draw_shape.fill_round(cx, Rect { pos: dvec2(search.pos.x + 9.0 + tw, search.pos.y + 4.0), size: dvec2(1.0, 12.0) }, color(p.accent, 1.0), 0.0);
        }
        let body_y = rect.pos.y + head;
        self.draw_shape.fill_round(cx, Rect { pos: dvec2(rect.pos.x + 1.0, body_y - 1.0), size: dvec2(rect.size.x - 2.0, 1.0) }, color(p.line, 1.0), 0.0);
        let body_h = rect.pos.y + rect.size.y - body_y;
        if self.index.is_none() {
            self.draw_text.color = color(p.muted, 1.0);
            self.draw_text.draw_abs(cx, dvec2(x0, body_y + 14.0), "Gathering every engine's presets…");
            return true;
        }
        // Compact (a small window): the details go first, then the left
        // column (the search and the tags still narrow it).
        let show_right = rect.size.x >= 560.0;
        let left_w = if rect.size.x >= 400.0 { LEFT_W } else { 0.0 };
        let left = Rect { pos: dvec2(rect.pos.x, body_y), size: dvec2(left_w, body_h) };
        let right = Rect { pos: dvec2(rect.pos.x + rect.size.x - if show_right { RIGHT_W } else { 0.0 }, body_y), size: dvec2(if show_right { RIGHT_W } else { 0.0 }, body_h) };
        let mid = Rect { pos: dvec2(left.pos.x + left.size.x, body_y), size: dvec2(right.pos.x - left.pos.x - left.size.x, body_h) };
        if left_w > 0.0 {
            self.draw_left(cx, &p, left);
        }
        let waiting = self.draw_mid(cx, &p, mid);
        if show_right {
            self.draw_right(cx, &p, right);
        }
        waiting || self.hover.is_some_and(|(i, _)| self.heard != Some(i))
    }

    fn draw_left(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette, r: Rect) {
        self.draw_shape.fill_round(cx, r, color(p.panel, 0.55), 0.0);
        self.draw_shape.fill_round(cx, Rect { pos: dvec2(r.pos.x + r.size.x - 1.0, r.pos.y), size: dvec2(1.0, r.size.y) }, color(p.line_faint, 1.0), 0.0);
        cx.begin_turtle(Walk::abs_rect(r), Layout::default().with_clip(true, true));
        self.left_rect = r;
        let mut y = r.pos.y + 8.0 - self.left_scroll;
        let y0 = y;
        let x = r.pos.x + 8.0;
        let w = r.size.x - 16.0;
        let row = |this: &mut Self, cx: &mut Cx2d, y: f64, text: &str, count: Option<usize>, on: bool, hit: Hit| {
            let rr = Rect { pos: dvec2(x - 2.0, y), size: dvec2(w + 4.0, SIDE_ROW - 1.0) };
            let under = this.mouse.is_some_and(|m| rr.contains(m));
            if on || under {
                this.draw_shape.fill_round(cx, rr, color(if on { p.accent_wash } else { p.control_hover }, 1.0), R_CONTROL);
            }
            this.draw_text.color = color(if on { p.accent } else { p.text }, 1.0);
            this.draw_text.draw_abs(cx, dvec2(x + 4.0, y + 3.0), text);
            if let Some(n) = count {
                let t = n.to_string();
                this.draw_small.color = color(p.muted, 1.0);
                let tw = advance(&this.draw_small, cx, &t);
                this.draw_small.draw_abs(cx, dvec2(x + w - tw - 2.0, y + 4.0), &t);
            }
            this.hit(rr, hit);
        };
        let sources: [(&str, Source, usize); 3] = [
            ("★ Favourites", Source::Favourites, self.favourites.len()),
            ("Recent", Source::Recent, self.recent.len()),
            ("Used in this score", Source::Used, self.used.len()),
        ];
        for (text, s, n) in sources {
            let on = self.source.as_ref() == Some(&s);
            row(self, cx, y, text, Some(n), on, Hit::Source(s));
            y += SIDE_ROW;
        }
        if let Some(Source::Similar(key)) = self.source.clone() {
            let label = self.by_key.get(&key).and_then(|i| self.entry(*i)).map_or_else(String::new, |e| format!("≈ {}", e.choice.label));
            let (label, _) = makepad_value_pickers::panel::control::fit_text(&self.draw_text, cx, &label, w - 8.0);
            row(self, cx, y, &label, None, true, Hit::Source(Source::Similar(key)));
            y += SIDE_ROW;
        }
        y += 8.0;
        self.draw_caps.color = color(p.muted, 1.0);
        self.draw_caps.draw_abs(cx, dvec2(x + 4.0, y), "ENGINES");
        y += 14.0;
        let all: usize = self.engines.iter().map(|(_, n)| n).sum();
        row(self, cx, y, "All engines", Some(all), self.facets.engine.is_none(), Hit::Engine(None));
        y += SIDE_ROW;
        for (engine, n) in self.engines.clone() {
            let on = self.facets.engine.as_deref() == Some(engine);
            let label = engine_name(engine);
            row(self, cx, y, label, Some(n), on, Hit::Engine(Some(engine)));
            y += SIDE_ROW;
        }
        y += 8.0;
        self.draw_caps.color = color(p.muted, 1.0);
        self.draw_caps.draw_abs(cx, dvec2(x + 4.0, y), "CATEGORIES");
        y += 14.0;
        for (cat, n) in self.categories.clone() {
            let on = self.facets.category.as_deref() == Some(cat);
            row(self, cx, y, cat, Some(n), on, Hit::Category(Some(cat)));
            y += SIDE_ROW;
        }
        // Packs, when there is more than one.
        if self.packs.len() > 1 {
            y += 8.0;
            self.draw_caps.color = color(p.muted, 1.0);
            self.draw_caps.draw_abs(cx, dvec2(x + 4.0, y), "PACKS");
            y += 14.0;
            for (pack, n) in self.packs.clone() {
                let on = self.facets.pack.as_deref() == Some(pack.as_str());
                let (label, _) = makepad_value_pickers::panel::control::fit_text(&self.draw_text, cx, &pack, w - 30.0);
                row(self, cx, y, &label, Some(n), on, Hit::Pack(Some(pack)));
                y += SIDE_ROW;
            }
        }
        // Libraries to download (their licence named before anything is
        // installed).
        let wanted: Vec<browse::Library> = browse::libraries().into_iter().filter(|l| !l.installed).collect();
        if !wanted.is_empty() {
            y += 8.0;
            self.draw_caps.color = color(p.muted, 1.0);
            self.draw_caps.draw_abs(cx, dvec2(x + 4.0, y), "LIBRARIES");
            y += 14.0;
            for lib in wanted {
                let label = format!("Get {} ({})", lib.title, lib.licence);
                let (label, _) = makepad_value_pickers::panel::control::fit_text(&self.draw_text, cx, &label, w - 8.0);
                row(self, cx, y, &label, None, false, Hit::Library(lib.engine.clone()));
                y += SIDE_ROW;
            }
        }
        // As far as it scrolls: its end at the bottom.
        let content = y - y0 + 8.0;
        self.left_scroll = self.left_scroll.min((content - r.size.y).max(0.0));
        cx.end_turtle();
    }

    /// The list: the tag chips, then a row per entry in view. True while
    /// a rested row waits to be heard.
    fn draw_mid(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette, r: Rect) -> bool {
        let x0 = r.pos.x + 10.0;
        // The character tags that narrow what shows.
        let mut x = x0;
        let ty = r.pos.y + 7.0;
        for (tag, _) in self.tags.clone() {
            let on = self.facets.tags.contains(&tag);
            let label = tag.replace('_', " ");
            let w = advance(&self.draw_small, cx, &label) + 12.0;
            if x + w > r.pos.x + r.size.x - 70.0 {
                break;
            }
            let rr = Rect { pos: dvec2(x, ty), size: dvec2(w, 17.0) };
            let under = self.mouse.is_some_and(|m| rr.contains(m));
            self.draw_shape.edged_round(cx, rr, color(if on { p.accent_wash } else if under { p.control_hover } else { p.control }, 1.0), color(if on { p.accent } else { p.line_faint }, 1.0), 1.0, 8.5);
            self.draw_small.color = color(if on { p.accent } else { p.label }, 1.0);
            self.draw_small.draw_abs(cx, dvec2(x + 6.0, ty + 3.0), &label);
            self.hit(rr, Hit::Tag(tag));
            x += w + 4.0;
        }
        let count = format!("{}", self.view.len());
        self.draw_small.color = color(p.muted, 1.0);
        let cw = advance(&self.draw_small, cx, &count);
        self.draw_small.draw_abs(cx, dvec2(r.pos.x + r.size.x - cw - 10.0, ty + 3.0), &count);
        let filtered = !self.query.is_empty() || self.facets != Facets::default() || self.source.is_some();
        if filtered {
            let cr = Rect { pos: dvec2(r.pos.x + r.size.x - cw - 56.0, ty), size: dvec2(40.0, 17.0) };
            self.draw_small.color = color(p.link, 1.0);
            self.draw_small.draw_abs(cx, dvec2(cr.pos.x + 4.0, cr.pos.y + 3.0), "clear");
            self.hit(cr, Hit::Clear);
        }
        let list = Rect { pos: dvec2(r.pos.x, r.pos.y + 30.0), size: dvec2(r.size.x, r.size.y - 31.0) };
        self.list = list;
        let total = self.view.len() as f64 * ROW;
        if self.reveal {
            let y = self.sel as f64 * ROW;
            if y < self.scroll {
                self.scroll = y;
            } else if y + ROW > self.scroll + list.size.y {
                self.scroll = y + ROW - list.size.y;
            }
            self.reveal = false;
        }
        self.scroll = self.scroll.clamp(0.0, (total - list.size.y).max(0.0));
        if self.view.is_empty() {
            self.draw_text.color = color(p.muted, 1.0);
            let msg = match self.source {
                Some(Source::Favourites) => "No favourites yet: the star on a row keeps one here.",
                Some(Source::Recent) => "Nothing picked yet.",
                _ => "Nothing matches.",
            };
            self.draw_text.draw_abs(cx, dvec2(x0, list.pos.y + 10.0), msg);
            return false;
        }
        cx.begin_turtle(Walk::abs_rect(list), Layout::default().with_clip(true, true));
        let first = (self.scroll / ROW).floor() as usize;
        let shown = (list.size.y / ROW).ceil() as usize + 1;
        let w = list.size.x;
        for at in first..(first + shown).min(self.view.len()) {
            let i = self.view[at];
            let Some(e) = self.entry(i).cloned() else { continue };
            let y = list.pos.y + at as f64 * ROW - self.scroll;
            let row = Rect { pos: dvec2(list.pos.x + 4.0, y), size: dvec2(w - 8.0, ROW - 2.0) };
            let key = e.choice.key();
            let current = self.current.as_deref() == Some(key.as_str());
            let selected = at == self.sel;
            let under = self.mouse.is_some_and(|m| row.contains(m) && list.contains(m));
            if selected || under {
                self.draw_shape.fill_round(cx, row, color(if selected { p.accent_wash } else { p.control_hover }, 1.0), R_CONTROL);
            }
            if current {
                self.draw_shape.fill_round(cx, Rect { pos: dvec2(row.pos.x + 2.0, y + 5.0), size: dvec2(2.0, ROW - 12.0) }, color(p.accent, 1.0), 1.0);
            }
            let thumb = Rect { pos: dvec2(row.pos.x + 10.0, y + 4.0), size: dvec2(40.0, ROW - 10.0) };
            self.draw_thumb(cx, p, &e, thumb);
            self.draw_text.color = color(if current { p.accent } else if selected { p.text_bright } else { p.text }, 1.0);
            let name_w = (w * 0.42).max(120.0);
            let (name, _) = makepad_value_pickers::panel::control::fit_text(&self.draw_text, cx, &e.choice.label, name_w);
            self.draw_text.draw_abs(cx, dvec2(thumb.pos.x + thumb.size.x + 10.0, y + 5.5), &name);
            let meta = format!("{} · {}", engine_name(e.engine), e.category);
            let mx = thumb.pos.x + thumb.size.x + 20.0 + name_w;
            self.draw_small.color = color(p.muted, 1.0);
            let (meta, _) = makepad_value_pickers::panel::control::fit_text(&self.draw_small, cx, &meta, (row.pos.x + row.size.x - 30.0 - mx).max(0.0));
            self.draw_small.draw_abs(cx, dvec2(mx, y + 7.0), &meta);
            // The star.
            let fav = self.favourites.contains(&key);
            let sr = Rect { pos: dvec2(row.pos.x + row.size.x - 22.0, y + 3.0), size: dvec2(18.0, ROW - 8.0) };
            if fav || under || selected {
                self.draw_small.color = color(if fav { p.amber } else { p.dim }, 1.0);
                self.draw_small.draw_abs(cx, dvec2(sr.pos.x + 4.0, y + 6.0), if fav { "★" } else { "☆" });
            }
            self.hit(row, Hit::Row(at));
            self.hit(sr, Hit::Star(i));
        }
        cx.end_turtle();
        // The scroll's place.
        if total > list.size.y {
            let h = (list.size.y * list.size.y / total).max(16.0);
            let t = self.scroll / (total - list.size.y);
            self.draw_shape.fill_round(cx, Rect { pos: dvec2(list.pos.x + list.size.x - 4.0, list.pos.y + t * (list.size.y - h)), size: dvec2(3.0, h) }, color(p.line_soft, 1.0), 1.5);
        }
        false
    }

    /// The selected entry: what it is, its settings, a kit's pads.
    fn draw_right(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette, r: Rect) {
        self.draw_shape.fill_round(cx, r, color(p.panel, 0.55), 0.0);
        self.draw_shape.fill_round(cx, Rect { pos: r.pos, size: dvec2(1.0, r.size.y) }, color(p.line_faint, 1.0), 0.0);
        let Some(i) = self.view.get(self.sel).copied() else { return };
        let Some(e) = self.entry(i).cloned() else { return };
        cx.begin_turtle(Walk::abs_rect(r), Layout::default().with_clip(true, true));
        let x = r.pos.x + 12.0;
        let w = r.size.x - 24.0;
        let mut y = r.pos.y + 10.0;
        let big = Rect { pos: dvec2(x, y), size: dvec2(w, 54.0) };
        self.draw_shape.edged_round(cx, big, color(p.well, 1.0), color(p.line_faint, 1.0), 1.0, R_CONTROL);
        self.draw_thumb(cx, p, &e, Rect { pos: big.pos + dvec2(6.0, 6.0), size: big.size - dvec2(12.0, 12.0) });
        y += 62.0;
        self.draw_title.color = color(p.text_bright, 1.0);
        let (name, _) = makepad_value_pickers::panel::control::fit_text(&self.draw_title, cx, &e.choice.label, w);
        self.draw_title.draw_abs(cx, dvec2(x, y), &name);
        y += 18.0;
        self.draw_small.color = color(p.secondary, 1.0);
        let meta = format!("{} · {} · {}", engine_name(e.engine), e.category, e.pack);
        let (meta, _) = makepad_value_pickers::panel::control::fit_text(&self.draw_small, cx, &meta, w);
        self.draw_small.draw_abs(cx, dvec2(x, y), &meta);
        y += 16.0;
        // What it is, wrapped.
        self.draw_text.color = color(p.text, 1.0);
        for line in wrap(&self.draw_text, cx, &e.about, w).into_iter().take(4) {
            self.draw_text.draw_abs(cx, dvec2(x, y), &line);
            y += 13.0;
        }
        y += 4.0;
        if !e.tags.is_empty() {
            self.draw_small.color = color(p.muted, 1.0);
            let tags: Vec<String> = e.tags.iter().take(8).map(|t| t.replace('_', " ")).collect();
            for line in wrap(&self.draw_small, cx, &tags.join(" · "), w).into_iter().take(2) {
                self.draw_small.draw_abs(cx, dvec2(x, y), &line);
                y += 12.0;
            }
            y += 4.0;
        }
        // A kit's voices: a pad each, played on a click.
        if let Thumb::Kit { slots } = e.thumb {
            self.draw_caps.color = color(p.muted, 1.0);
            self.draw_caps.draw_abs(cx, dvec2(x, y), "VOICES (a click plays one)");
            y += 12.0;
            let cols = 4.0;
            let pw = (w - 3.0 * (cols - 1.0)) / cols;
            for (s, name) in makepad_score_player::presets::DRUM_SLOTS.iter().enumerate() {
                let (c, rr) = ((s % 4) as f64, (s / 4) as f64);
                let pad = Rect { pos: dvec2(x + c * (pw + 3.0), y + rr * 21.0), size: dvec2(pw, 18.0) };
                let filled = slots & (1 << s) != 0;
                let under = self.mouse.is_some_and(|m| pad.contains(m));
                self.draw_shape.edged_round(cx, pad, color(if under && filled { p.control_hover } else if filled { p.raised } else { p.well }, 1.0), color(p.line_faint, 1.0), 1.0, 3.0);
                self.draw_caps.color = color(if filled { p.text } else { p.dim }, 1.0);
                let label = name.replace('_', " ").to_uppercase();
                let tw = advance(&self.draw_caps, cx, &label);
                self.draw_caps.draw_abs(cx, dvec2(pad.pos.x + (pad.size.x - tw) * 0.5, pad.pos.y + 5.0), &label);
                if filled {
                    self.hit(pad, Hit::Pad(i, name));
                }
            }
            y += 4.0 * 21.0 + 6.0;
        } else {
            // Its settings, as it will be written.
            self.draw_caps.color = color(p.muted, 1.0);
            self.draw_caps.draw_abs(cx, dvec2(x, y), "WRITES");
            y += 12.0;
            let fields: Vec<String> = e.choice.fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
            let code = format!("{}{{{}}}", e.choice.type_name, fields.join(" "));
            self.draw_small.color = color(p.text_soft, 1.0);
            for line in wrap(&self.draw_small, cx, &code, w).into_iter().take(3) {
                self.draw_small.draw_abs(cx, dvec2(x, y), &line);
                y += 12.0;
            }
            y += 6.0;
        }
        let bx = x;
        let a = self.button(cx, p, dvec2(bx, y), "▶ Audition", self.heard == Some(i), true, Hit::Audition(i));
        let s = self.button(cx, p, dvec2(a.pos.x + a.size.x + 4.0, y), "≈ Similar", matches!(self.source, Some(Source::Similar(_))), true, Hit::Similar(i));
        let fav = self.favourites.contains(&e.choice.key());
        self.button(cx, p, dvec2(s.pos.x + s.size.x + 4.0, y), if fav { "★" } else { "☆" }, fav, true, Hit::Star(i));
        cx.end_turtle();
    }

    /// An entry's small picture in `r`.
    fn draw_thumb(&mut self, cx: &mut Cx2d, p: &makepad_look::palette::Palette, e: &Entry, r: Rect) {
        let ink = color(p.info, 0.95);
        let dot = |this: &mut Self, cx: &mut Cx2d, at: DVec2, c: Vec4f| this.draw_shape.fill_round(cx, Rect { pos: at - dvec2(0.75, 0.75), size: dvec2(1.5, 1.5) }, c, 0.0);
        let n = r.size.x.max(2.0) as usize;
        match &e.thumb {
            Thumb::Env { adsr, wave } => {
                let [a, d, s, rel] = *adsr;
                let total = (a + d + 0.3 + rel).max(1e-3);
                for i in 0..n {
                    let t = i as f32 / n as f32 * total;
                    let v = if t < a {
                        t / a.max(1e-4)
                    } else if t < a + d {
                        1.0 - (1.0 - s) * ((t - a) / d.max(1e-4))
                    } else if t < a + d + 0.3 {
                        s
                    } else {
                        s * (1.0 - ((t - a - d - 0.3) / rel.max(1e-4)).min(1.0))
                    };
                    // The wave inside the envelope.
                    let ph = i as f64 / n as f64 * 6.0;
                    let f = ph.fract();
                    let w = match wave {
                        0 => (ph * std::f64::consts::TAU).sin(),
                        1 => 1.0 - 4.0 * (f - 0.5).abs(),
                        3 | 4 => if f < 0.5 { 0.8 } else { -0.8 },
                        5 => (((ph * 37.0).sin() * 4375.85).fract() * 2.0 - 1.0) * 0.8,
                        _ => 1.0 - 2.0 * f,
                    };
                    let mid = r.pos.y + r.size.y * 0.5;
                    let amp = v as f64 * r.size.y * 0.48;
                    dot(self, cx, dvec2(r.pos.x + i as f64, mid - w * amp), color(p.info, 0.55));
                    dot(self, cx, dvec2(r.pos.x + i as f64, mid - amp), ink);
                }
            }
            Thumb::Table { partials } => {
                let mid = r.pos.y + r.size.y * 0.5;
                let norm: f32 = partials.iter().sum::<f32>().max(1e-3);
                for i in 0..n {
                    let ph = i as f32 / n as f32 * 2.0 * std::f32::consts::TAU;
                    let v: f32 = partials.iter().enumerate().map(|(h, a)| a * (ph * (h + 1) as f32).sin()).sum::<f32>() / norm * 1.6;
                    dot(self, cx, dvec2(r.pos.x + i as f64, mid - v.clamp(-1.0, 1.0) as f64 * r.size.y * 0.45), color(p.teal, 0.95));
                }
            }
            Thumb::Kit { slots } => {
                let (cols, rows) = (8.0, 2.0);
                let cw = r.size.x / cols;
                let ch = r.size.y / rows;
                for s in 0..16 {
                    let c = (s % 8) as f64;
                    let rr = (s / 8) as f64;
                    let on = slots & (1 << s) != 0;
                    self.draw_shape.fill_round(cx, Rect { pos: dvec2(r.pos.x + c * cw + 0.5, r.pos.y + rr * ch + 0.5), size: dvec2(cw - 1.0, ch - 1.0) }, color(if on { p.warm } else { p.edge_lo }, if on { 0.85 } else { 1.0 }), 1.0);
                }
            }
            Thumb::Mark => {
                // The category's initial in a quiet tile.
                self.draw_shape.edged_round(cx, r, color(p.raised, 1.0), color(p.line_faint, 1.0), 1.0, 3.0);
                let t: String = e.category.chars().take(4).collect::<String>().to_uppercase();
                self.draw_caps.color = color(p.muted, 1.0);
                let tw = advance(&self.draw_caps, cx, &t);
                self.draw_caps.draw_abs(cx, dvec2(r.pos.x + (r.size.x - tw) * 0.5, r.pos.y + (r.size.y - 8.0) * 0.5), &t);
            }
        }
    }
}

/// An engine as people call it.
fn engine_name(engine: &str) -> &str {
    match engine {
        "GM" => "General MIDI",
        "Mine" => "My instruments",
        "Drums" => "Drum kits",
        e => e,
    }
}

/// `text` broken into lines `width` wide.
fn wrap(draw: &DrawText, cx: &mut Cx2d, text: &str, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let next = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if advance(draw, cx, &next) > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = next;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// What an [`InstrumentBrowser`] did, for its holder.
#[derive(Clone, Debug, Default)]
pub enum InstrumentBrowserAction {
    #[default]
    None,
    Close,
    /// Pop out (`true`) or dock back.
    Pop(bool),
}

/// The browser as a widget: a window's whole content, or a panel's.
#[derive(Script, ScriptHook, Widget)]
pub struct InstrumentBrowser {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    pub browser: Browser,
    #[rust]
    next_frame: NextFrame,
}

impl InstrumentBrowser {
    /// Take the keys (its window came to the front).
    pub fn focus(&self, cx: &mut Cx) {
        cx.set_key_focus(self.draw_bg.area());
    }

    /// Bind it to `node` and open it.
    pub fn open(&mut self, cx: &mut Cx, node: &str, popped: bool) {
        self.browser.popped = popped;
        self.browser.open(node);
        self.redraw(cx);
    }
}

impl Widget for InstrumentBrowser {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() {
            if self.browser.tick() {
                self.next_frame = cx.new_next_frame();
            }
            self.redraw(cx);
        }
        // Keys only while it has the keys (its window's, a click on it).
        let keys = matches!(event, Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_));
        if keys && !cx.has_key_focus(self.draw_bg.area()) {
            return;
        }
        if let Event::MouseDown(e) = event {
            if self.browser.rect.contains(e.abs) {
                cx.set_key_focus(self.draw_bg.area());
            } else {
                return;
            }
        }
        let out = self.browser.handle_event(cx, event);
        if out.redraw {
            self.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
        if out.close {
            cx.widget_action(self.uid, InstrumentBrowserAction::Close);
        }
        if let Some(pop) = out.pop {
            cx.widget_action(self.uid, InstrumentBrowserAction::Pop(pop));
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        if self.browser.draw(cx, rect) {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }
}
