//! The devhub panel's window (apps/devhub_panel's `DevhubPanel`: every box's leases and load on
//! one time axis, what needs a look, the queue, the landing strip), the menu-bar item's Open panel.
//! It is devhub's own window, in devhub's own process: on the Mac the menu-bar item's helper
//! ([`crate::menu_item`]), which never execs, so the master's self-updates never close it;
//! elsewhere the master. 11-24: the panel was an app of its own with a second menu-bar item.
//!
//! Whether it shows and where is kept in devhub's home ([`kept`]), as it opens and closes. A helper
//! that starts while it showed opens it there again ([`PanelWindow::restore`]): the next helper of
//! a self-update, which starts beside the one it replaces, a helper started again after a crash, a
//! login. Its window shows once its panel has a fleet, so it never shows an empty page over the old
//! one's, and only while it is still kept shown then: the helper it replaces keeps it until then,
//! and a close there meanwhile holds. The helper then says it is ready: the master ends the old one
//! ([`Said::Ready`]). A restore makes its window and never activates devhub: it takes no focus.
//!
//! Closed, the window hides and the panel asks the master nothing.

use makepad_app_devhub_panel::panel::DevhubPanel;
use makepad_strict_json::{obj, Value};
use makepad_widgets::*;
use std::path::{Path, PathBuf};

/// What the panel's window did, for the master's log (and its helper's handover).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Said {
    Opened,
    Closed,
    /// what it showed at its start shows: the helper it replaces may go
    Ready,
}

/// Where the panel's window is kept: whether it shows, and its frame.
pub fn kept() -> PathBuf {
    makepad_devhub::proto::home().join("panel-window.json")
}

/// The kept window: shown, and its position and inner size (None: none kept).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kept {
    pub shown: bool,
    pub frame: Option<(Vec2d, Vec2d)>,
}

impl Kept {
    pub fn read(path: &Path) -> Kept {
        let v = std::fs::read(path).ok().and_then(|b| makepad_strict_json::parse(&b).ok());
        let f = |k: &str| match v.as_ref().and_then(|v| v.get(k)) {
            Some(Value::F64(x)) => Some(*x),
            Some(Value::Int(i)) => Some(*i as f64),
            _ => None,
        };
        let frame = match (f("x"), f("y"), f("w"), f("h")) {
            (Some(x), Some(y), Some(w), Some(h)) if w >= 1.0 && h >= 1.0 => Some((dvec2(x, y), dvec2(w, h))),
            _ => None,
        };
        Kept { shown: v.as_ref().and_then(|v| v.get("shown")).and_then(Value::as_bool) == Some(true), frame }
    }

    fn text(&self) -> String {
        let mut fields = vec![("shown", Value::Bool(self.shown))];
        if let Some((p, s)) = self.frame {
            fields.extend([("x", Value::F64(p.x)), ("y", Value::F64(p.y)), ("w", Value::F64(s.x)), ("h", Value::F64(s.y))]);
        }
        obj(fields).to_json()
    }
}

#[derive(Default)]
pub struct PanelWindow {
    /// where it is kept (None: nowhere, the panel alone in a window: `MAKEPAD_DEVHUB_PANEL=1`)
    keep: Option<PathBuf>,
    shown: bool,
    /// opened at start: its window shows once the panel has a fleet
    waiting: bool,
    /// shown for a restore: ready once its window drew (after its first geometry)
    readying: Option<bool>,
    /// opened by [`PanelWindow::restore`]: it shows only if it is still kept shown then
    restored: bool,
    /// its frame moved since it was kept: kept on `keep_timer`
    moved: bool,
    keep_timer: Timer,
    said: Vec<Said>,
}

impl PanelWindow {
    pub fn new(keep: Option<PathBuf>) -> PanelWindow {
        PanelWindow { keep, ..PanelWindow::default() }
    }

    /// Opens the panel where it was kept when it showed then; its window shows once the panel has
    /// a fleet. False: nothing to open.
    pub fn restore(&mut self, cx: &mut Cx, ui: &WidgetRef) -> bool {
        let Some(path) = &self.keep else { return false };
        let kept = Kept::read(path);
        if !kept.shown {
            return false;
        }
        self.place(cx, ui, kept.frame);
        set_shown(cx, ui, true);
        self.shown = true;
        self.waiting = true;
        self.restored = true;
        eprintln!("devhub: the panel shows again where it was ({:?}) once it has a fleet", kept.frame);
        true
    }

    /// Opens the window, or brings it to the front.
    pub fn open(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        let window = ui.window(cx, ids!(panel_window));
        if !self.shown {
            if window.window_id().is_none_or(|id| !cx.windows[id].is_created) {
                let frame = self.keep.as_deref().and_then(|p| Kept::read(p).frame);
                self.place(cx, ui, frame);
            }
            set_shown(cx, ui, true);
            self.shown = true;
            self.said.push(Said::Opened);
        }
        self.waiting = false;
        self.restored = false;
        window.show(cx);
        self.keep(cx, ui);
    }

    /// Hides the window: the panel stops asking.
    pub fn close(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        if !self.shown {
            return;
        }
        ui.window(cx, ids!(panel_window)).set_visible(cx, false);
        set_shown(cx, ui, false);
        self.shown = false;
        self.waiting = false;
        self.restored = false;
        if self.readying.take().is_some() {
            self.said.push(Said::Ready);
        }
        self.said.push(Said::Closed);
        self.keep(cx, ui);
    }

    /// Its window's events (before the UI's): a close hides it when `hide_on_close` (the app lives
    /// in the menu bar), a move is kept, and a restored panel shows once it has a fleet.
    pub fn handle_event(&mut self, cx: &mut Cx, ui: &WidgetRef, event: &Event, hide_on_close: bool) {
        let id = ui.window(cx, ids!(panel_window)).window_id();
        match event {
            Event::WindowCloseRequested(e) if Some(e.window_id) == id && hide_on_close => {
                e.accept_close.set(false);
                self.close(cx, ui);
            }
            // (closed for good, where the app is not in a menu bar: it is made again on the next open)
            Event::WindowClosed(e) if Some(e.window_id) == id && self.shown => {
                set_shown(cx, ui, false);
                self.shown = false;
                self.waiting = false;
                self.readying = None;
                self.restored = false;
                self.said.push(Said::Closed);
                if let Some(path) = &self.keep {
                    let frame = Kept::read(path).frame;
                    let _ = std::fs::write(path, Kept { shown: false, frame }.text());
                }
            }
            Event::WindowGeomChange(e) if Some(e.window_id) == id => {
                if self.readying == Some(false) {
                    self.readying = Some(true);
                }
                if self.shown && !self.moved {
                    self.moved = true;
                    self.keep_timer = cx.start_timeout(1.0);
                }
            }
            Event::Timer(t) if self.keep_timer.is_timer(t).is_some() => self.keep(cx, ui),
            Event::Signal if self.waiting && has_fleet(cx, ui) => {
                self.waiting = false;
                if std::mem::take(&mut self.restored) && !self.keep.as_deref().is_some_and(|p| Kept::read(p).shown) {
                    set_shown(cx, ui, false);
                    self.shown = false;
                    eprintln!("devhub: the panel was closed while this helper started: it stays closed");
                    self.said.push(Said::Ready);
                    return;
                }
                self.readying = Some(false);
                ui.window(cx, ids!(panel_window)).show(cx);
            }
            _ => {}
        }
    }

    /// After the UI drew `event`: a restored window that drew its first frame is ready.
    pub fn after_event(&mut self, event: &Event) {
        if let (Event::Draw(_), Some(true)) = (event, self.readying) {
            self.readying = None;
            self.said.push(Said::Ready);
        }
    }

    /// What it did since the last call.
    pub fn said(&mut self) -> Vec<Said> {
        std::mem::take(&mut self.said)
    }

    /// Where the window opens when it is made: the kept frame.
    fn place(&self, cx: &mut Cx, ui: &WidgetRef, frame: Option<(Vec2d, Vec2d)>) {
        if let Some((position, size)) = frame {
            ui.window(cx, ids!(panel_window)).configure_window(cx, size, position, false, "devhub panel".into());
        }
    }

    /// Keeps whether it shows and its frame (a small file, at most once a second while it moves).
    fn keep(&mut self, cx: &mut Cx, ui: &WidgetRef) {
        self.moved = false;
        let Some(path) = &self.keep else { return };
        let window = ui.window(cx, ids!(panel_window));
        let created = window.window_id().is_some_and(|id| cx.windows[id].is_created);
        let frame = if created { Some((window.get_position(cx), window.get_inner_size(cx))) } else { Kept::read(path).frame };
        let text = Kept { shown: self.shown, frame }.text();
        if let Err(e) = std::fs::write(path, text) {
            eprintln!("devhub: the panel's window is not kept: {}: {e}", path.display());
        }
    }
}

fn set_shown(cx: &mut Cx, ui: &WidgetRef, shown: bool) {
    if let Some(mut p) = ui.widget(cx, ids!(fleet_panel)).borrow_mut::<DevhubPanel>() {
        p.set_shown(cx, shown);
    }
}

fn has_fleet(cx: &mut Cx, ui: &WidgetRef) -> bool {
    ui.widget(cx, ids!(fleet_panel)).borrow::<DevhubPanel>().is_some_and(|p| p.has_fleet())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kept window reads back as it was written; a file without a whole frame keeps none.
    #[test]
    fn the_panels_window_is_kept_whole() {
        let dir = std::env::temp_dir().join(format!("devhub-panel-kept-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("panel-window.json");
        let kept = Kept { shown: true, frame: Some((dvec2(-1200.5, 40.0), dvec2(1440.0, 860.0))) };
        std::fs::write(&path, kept.text()).unwrap();
        assert_eq!(Kept::read(&path), kept);
        std::fs::write(&path, "{\"shown\":true,\"x\":3}").unwrap();
        assert_eq!(Kept::read(&path), Kept { shown: true, frame: None });
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Kept::read(&path), Kept { shown: false, frame: None }, "nothing kept: nothing shows");
    }
}
