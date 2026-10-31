//! Instrument browsers popped out of their cards: each its own small
//! window that stays on top of the others, bound to one instrument node,
//! so several can be open at once to try instruments on several tracks
//! while the song plays. Each picks live (no restart of playback), with
//! A/B and revert of its own; its title says which node it drives.
//!
//! A window follows its node: an edit keeps it, a rename moves it to the
//! node now in the same place among the instruments. Where the windows are
//! is kept per score and they come back when the score opens again.

use super::Desk;
use makepad_synth_panel::browser::{InstrumentBrowser, InstrumentBrowserAction};
use makepad_widgets::*;

/// A popped-out browser.
pub struct BrowserWindow {
    pub node: String,
    /// The node's place among the score's instruments (to follow a rename).
    ordinal: usize,
    window: WidgetRef,
    drawn: bool,
    closing: bool,
    /// Where it is (position, size), as last moved.
    place: Option<(DVec2, DVec2)>,
}

/// The pop-outs of the open score.
#[derive(Default)]
pub struct BrowserWindows {
    pub open: Vec<BrowserWindow>,
    /// The score their places were restored for.
    restored: Option<String>,
    /// The revision the bindings were checked at.
    checked: u64,
}

const FIRST_SIZE: DVec2 = DVec2 { x: 640.0, y: 420.0 };

/// A file name for a score's places.
fn places_path(score: &str) -> std::path::PathBuf {
    crate::paths::data_dir().join("prefs").join(format!("browsers-{}", makepad_score_live::source::fnv_hex(score)))
}

impl Desk {
    /// The score's key for its windows' places: its file, or "draft".
    fn browsers_key(&self) -> String {
        self.document.path.as_ref().map_or_else(|| "draft".to_string(), |p| p.to_string_lossy().into_owned())
    }

    /// Pop the browser for `node` out (or bring its window forward).
    pub(super) fn pop_browser(&mut self, cx: &mut Cx, _ui: &WidgetRef, node: &str) {
        if self.browsers.open.iter().any(|b| b.node == node && !b.closing) {
            return;
        }
        let place = self.saved_places().into_iter().find(|(n, ..)| n == node).map(|(_, p, s)| (p, s));
        self.open_browser_window(cx, node, place);
        self.save_places();
    }

    fn open_browser_window(&mut self, cx: &mut Cx, node: &str, place: Option<(DVec2, DVec2)>) {
        let title = self.browser_title(node);
        let window = cx.with_vm(|vm| {
            let value = script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                Window{
                    window.title: "Instrument"
                    window.topmost: true
                    body +: {
                        width: Fill
                        height: Fill
                        browser := InstrumentBrowser{}
                    }
                }
            });
            WidgetRef::script_from_value(vm, value)
        });
        if let Some(id) = window.as_window().window_id() {
            if cx.windows.is_valid(id) {
                let w = &mut cx.windows[id];
                w.create_inner_size = Some(place.map_or(FIRST_SIZE, |(_, s)| s));
                if let Some((pos, _)) = place {
                    w.create_position = Some(pos);
                }
            }
        }
        window.as_window().set_title(cx, &title);
        if let Some(mut b) = window.widget(cx, ids!(browser)).borrow_mut::<InstrumentBrowser>() {
            b.open(cx, node, true);
        }
        let ordinal = self.doc.as_ref().and_then(|d| d.insts.iter().position(|i| i.name == node)).unwrap_or(0);
        self.browsers.open.push(BrowserWindow { node: node.to_string(), ordinal, window, drawn: false, closing: false, place });
    }

    /// "keys · Synth trance lead": the node and what it is now.
    fn browser_title(&self, node: &str) -> String {
        let what = makepad_score_live::instruments::current_key(&self.source, node).map(|k| {
            let (ty, fields) = k.split_once(':').unwrap_or((&k, ""));
            let fields: Vec<String> = fields.split(',').filter_map(|f| f.split_once('=')).map(|(_, v)| v.trim_start_matches('@').trim_matches('"').replace('_', " ")).collect();
            format!("{ty} {}", fields.join(" "))
        });
        match what {
            Some(what) => format!("{node} · {what}"),
            None => node.to_string(),
        }
    }

    fn saved_places(&self) -> Vec<(String, DVec2, DVec2)> {
        let Ok(text) = std::fs::read_to_string(places_path(&self.browsers_key())) else { return Vec::new() };
        text.lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('\t').collect();
                let n = |i: usize| parts.get(i).and_then(|v| v.parse::<f64>().ok());
                Some((parts.first()?.to_string(), dvec2(n(1)?, n(2)?), dvec2(n(3)?, n(4)?)))
            })
            .collect()
    }

    /// Keep where the open windows are, for this score.
    fn save_places(&self) {
        let lines: Vec<String> = self
            .browsers
            .open
            .iter()
            .filter(|b| !b.closing)
            .map(|b| {
                let (p, s) = b.place.unwrap_or((DVec2::default(), FIRST_SIZE));
                format!("{}\t{}\t{}\t{}\t{}", b.node, p.x, p.y, s.x, s.y)
            })
            .collect();
        let path = places_path(&self.browsers_key());
        if lines.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, lines.join("\n"));
    }

    /// Every turn: the windows of a score that just opened come back, and
    /// each follows its node through edits and renames.
    pub(super) fn pump_browsers(&mut self, cx: &mut Cx) {
        let key = self.browsers_key();
        if self.browsers.restored.as_deref() != Some(key.as_str()) && self.doc.is_some() {
            // Another score: its own windows (the last score's close).
            for b in &mut self.browsers.open {
                b.closing = true;
                b.window.as_window().close(cx);
            }
            self.browsers.restored = Some(key);
            for (node, pos, size) in self.saved_places() {
                if self.doc.as_ref().is_some_and(|d| d.inst(&node).is_some()) {
                    self.open_browser_window(cx, &node, Some((pos, size)));
                }
            }
        }
        if self.browsers.checked == self.revision {
            return;
        }
        self.browsers.checked = self.revision;
        let Some(doc) = self.doc.as_ref() else { return };
        let names: Vec<String> = doc.insts.iter().map(|i| i.name.clone()).collect();
        let mut renamed = false;
        let bound: Vec<String> = self.browsers.open.iter().map(|b| b.node.clone()).collect();
        let mut titles = Vec::new();
        for b in &mut self.browsers.open {
            if let Some(at) = names.iter().position(|n| *n == b.node) {
                b.ordinal = at;
            } else if let Some(next) = names.get(b.ordinal).filter(|n| !bound.contains(n)) {
                // Renamed: the node now in its place.
                b.node = next.clone();
                renamed = true;
            }
            titles.push(b.node.clone());
        }
        for (i, node) in titles.into_iter().enumerate() {
            let title = self.browser_title(&node);
            let b = &self.browsers.open[i];
            b.window.as_window().set_title(cx, &title);
            if let Some(mut w) = b.window.widget(cx, ids!(browser)).borrow_mut::<InstrumentBrowser>() {
                if w.browser.node != node {
                    w.browser.rebind(&node);
                }
                w.browser.refresh();
            }
            b.window.redraw(cx);
        }
        if renamed {
            self.save_places();
        }
    }

    /// The windows' events and drawing (after the main window's).
    pub(super) fn browser_windows_event(&mut self, cx: &mut Cx, ui: &WidgetRef, event: &Event) {
        if self.browsers.open.is_empty() {
            return;
        }
        let mut changed = false;
        let mut dock = None;
        let mut i = 0;
        while i < self.browsers.open.len() {
            let window = self.browsers.open[i].window.clone();
            let id = window.as_window().window_id();
            match event {
                Event::WindowClosed(closed) if Some(closed.window_id) == id => {
                    let gone = self.browsers.open.remove(i);
                    if !gone.closing {
                        changed = true;
                    }
                    makepad_synth_panel::host::preview(&gone.node, None);
                    continue;
                }
                Event::WindowGeomChange(change) if Some(change.window_id) == id => {
                    let g = &change.new_geom;
                    if g.inner_size.x > 0.0 && g.inner_size.y > 0.0 {
                        let place = Some((g.position, g.inner_size));
                        if self.browsers.open[i].place != place {
                            self.browsers.open[i].place = place;
                            changed = true;
                        }
                    }
                    window.redraw(cx);
                }
                Event::WindowGotFocus(focus) if Some(*focus) == id => {
                    if let Some(b) = window.widget(cx, ids!(browser)).borrow::<InstrumentBrowser>() {
                        b.focus(cx);
                    }
                    // Its search has the keys (a space types, it does not play).
                    makepad_synth_panel::host::set_searching(true);
                }
                Event::WindowLostFocus(focus) if Some(*focus) == id => makepad_synth_panel::host::set_searching(false),
                _ => {}
            }
            if let Event::Draw(draw) = event {
                let created = id.is_some_and(|id| cx.windows.is_valid(id) && cx.windows[id].is_created);
                let mut cx_draw = CxDraw::new(cx, draw);
                window.draw_all(&mut Cx2d::new(&mut cx_draw), &mut Scope::empty());
                if created && draw.redraw_all {
                    self.browsers.open[i].drawn = true;
                }
                i += 1;
                continue;
            }
            let actions = cx.capture_actions(|cx| window.handle_event(cx, event, &mut Scope::empty()));
            let browser = window.widget(cx, ids!(browser));
            for action in actions.filter_widget_actions_cast::<InstrumentBrowserAction>(browser.widget_uid()) {
                match action {
                    InstrumentBrowserAction::Close => {
                        self.browsers.open[i].closing = true;
                        window.as_window().close(cx);
                        changed = true;
                    }
                    InstrumentBrowserAction::Pop(false) => {
                        // Docked: back under its card.
                        self.browsers.open[i].closing = true;
                        window.as_window().close(cx);
                        changed = true;
                        dock = Some(self.browsers.open[i].node.clone());
                    }
                    _ => {}
                }
            }
            if !self.browsers.open[i].drawn && id.is_some_and(|id| cx.windows.is_valid(id) && cx.windows[id].is_created) {
                cx.redraw_all();
            }
            i += 1;
        }
        if changed {
            self.save_places();
        }
        if let Some(node) = dock {
            if let Some(mut editor) = ui.widget(cx, ids!(score_code)).borrow_mut::<super::ScoreEditor>() {
                editor.open_browser(cx, &node);
            }
        }
    }
}
