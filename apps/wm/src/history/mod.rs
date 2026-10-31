//! Layout time-scrub. Every settled change of the window arrangement is
//! snapshotted (a ring of 200, kept in `MAKEPAD_HOME/wm/layouts`); SUPER plus
//! the wheel over the bare desktop steps the windows back and forward through
//! them with the usual tweens. Letting go of SUPER keeps what is shown (the
//! next settle stores it as a new layout), Escape returns to where you were.
//!
//! A snapshot holds the floating windows (desktop-style windows, or floats of
//! a tiling style): desk rect, minimized, maximized, snap zone, parked state,
//! stacking, focus, and the desktop space each is on. The tile tree of a
//! tiling style is not recorded, so tiled windows are not moved by a scrub.
//! Each desktop space has a history of its own: a scrub steps through the
//! arrangements of the desktop it starts on, and leaves the others alone.
//!
//! The newest snapshot is also how the spaces survive a restart: the desktops
//! are made again at startup and each window that comes back (matched by app
//! and title, `store::resolve`) goes to the desktop it was on, with the state
//! it had. Fullscreen spaces are not kept: their window counts for the desktop
//! it came from.
//!
//! Settling is observed, not hooked: the 1 s tick, a button-up and a key-up
//! compare the arrangement with the newest stored one, and a change that
//! stays for [`SETTLE`] is stored. Nothing runs per frame.
pub mod store;

use makepad_app_host::ClientId;
use crate::layout::LRect;
use crate::shell::{osd::OsdShow, ui::Ico};
use crate::App;
use makepad_widgets::*;
use std::path::PathBuf;
use store::{ago, parse, resolve, serialise, Live, Ring, Snapshot, Win};

/// A change must stay this long (s) to count as settled.
pub const SETTLE: f64 = 0.4;
/// The clients are told their new size this long (s) after the last step.
pub const RESIZE_DELAY: f64 = 0.25;
/// The OSD stays up this long (s); the end of the scrub hides it.
const OSD_SECS: f64 = 30.0;
/// Wheel units per step.
const STEP: f64 = 1.0;

struct Scrub {
    /// The arrangement when the scrub began: the way back.
    base: Snapshot,
    /// Ring indices visited, oldest first.
    list: Vec<usize>,
    /// The desktop (ordinal) whose history this is.
    space: usize,
    /// The position that shows `base` (the live layout).
    start: usize,
    cursor: usize,
    acc: f64,
}

#[derive(Default)]
pub struct History {
    ring: Ring,
    /// The newest change seen and when, until it settles.
    seen: Option<Snapshot>,
    settle: Timer,
    resume: Timer,
    scrub: Option<Scrub>,
    /// Clients keep their size until then (a scrub in progress).
    hold_until: f64,
    /// Windows of the last run, still waiting for their client to come back,
    /// and when to give up waiting (`host::now`).
    pending: Vec<Win>,
    pending_until: f64,
    /// Clients already put in their place: never moved again by this.
    placed: Vec<ClientId>,
    /// The file as this WM last read or wrote it: a file changed since (somebody
    /// edited it, another WM wrote it) is read again before the next write, so an
    /// external edit is never written over.
    disk: Option<std::time::SystemTime>,
}

fn modified(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// An example or test app (`examples/*` in the registry, e.g. counter): never part of the
/// layout history, so a restart never waits for or places one the user did not open.
fn is_example(app: &str) -> bool {
    makepad_app_registry::find(app).is_some_and(|entry| entry.dir.starts_with("examples/"))
}

/// How long (s) a restored window is waited for.
const RESTORE_SECS: f64 = 600.0;

fn file() -> PathBuf {
    crate::theme::makepad_home().join("wm/layouts")
}

impl History {
    pub fn load() -> Self {
        let path = file();
        let mut ring = std::fs::read_to_string(&path).map(|t| parse(&t)).unwrap_or_default();
        ring.retain_wins(|w| !is_example(&w.app));
        Self { ring, disk: modified(&path), ..Default::default() }
    }

    /// Store `snap` as the newest layout and write the file. The file is read again
    /// first when it changed on disk since this WM last touched it.
    fn store(&mut self, snap: Snapshot) {
        self.store_at(&file(), snap)
    }

    fn store_at(&mut self, path: &std::path::Path, snap: Snapshot) {
        let path = path.to_path_buf();
        if modified(&path) != self.disk {
            self.ring = std::fs::read_to_string(&path).map(|t| parse(&t)).unwrap_or_default();
            self.ring.retain_wins(|w| !is_example(&w.app));
        }
        if !self.ring.push(snap) {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("new");
        if std::fs::write(&tmp, serialise(&self.ring)).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            self.disk = modified(&path);
        }
    }

    /// A scrub is running, or just stepped: the desk keeps the clients at
    /// their size so a fast scrub does not resize them at every step.
    pub fn holds_resize(&self, now: f64) -> bool {
        self.scrub.is_some() || now < self.hold_until
    }

    pub fn scrubbing(&self) -> bool {
        self.scrub.is_some()
    }
}

impl App {
    fn history_live(&mut self) -> Vec<Live> {
        let state = self.state_mut();
        let mut out = Vec::new();
        for s in &state.screens.screens {
            for c in s.layout.all_clients() {
                if let Some(slot) = state.clients.get(&c).filter(|s| !s.warm && !s.is_preview && !is_example(&s.app)) {
                    out.push(Live { client: c, app: slot.app.clone(), title: slot.title.clone() });
                }
            }
        }
        out
    }

    /// The desktop ordinal `client` counts for on layout `l`: its own desktop,
    /// or for a fullscreen window the desktop it came from.
    fn desktop_of(l: &crate::layout::WmLayout, client: ClientId) -> usize {
        let Some(slot) = l.workspace_of(client) else { return 0 };
        let Some(i) = l.spaces.index_of_slot(slot) else { return 0 };
        let desk = match l.spaces.home_of(i) {
            Some(home) => l.spaces.index_of_slot(home).unwrap_or(0),
            None => i,
        };
        l.spaces.desktop_number(desk).map_or(0, |n| n - 1)
    }

    /// The ordinal of the desktop the active screen is on (for a fullscreen
    /// space, the one it came from).
    fn history_space(&mut self) -> usize {
        let l = self.state_mut().layout();
        let cur = l.spaces.cur();
        let desk = match l.spaces.home_of(cur) {
            Some(home) => l.spaces.index_of_slot(home).unwrap_or(0),
            None => cur,
        };
        l.spaces.desktop_number(desk).map_or(0, |n| n - 1)
    }

    /// The floating windows of every screen as they are now.
    fn history_capture(&mut self) -> Snapshot {
        let cur = self.history_space();
        let state = self.state_mut();
        let style = state.style.target as u8;
        let desktops = state.screens.screens.iter().map(|s| s.layout.spaces.desktop_count()).max().unwrap_or(1);
        let mut wins = Vec::new();
        for s in &state.screens.screens {
            let l = &s.layout;
            let focus = l.focused_client();
            let mut add = |client: ClientId, rect: LRect, minimized, maximized, parked, snap| {
                let Some(slot) = state.clients.get(&client).filter(|s| !s.warm && !s.is_preview && !is_example(&s.app)) else {
                    return;
                };
                wins.push(Win {
                    app: slot.app.clone(),
                    title: slot.title.clone(),
                    client: Some(client),
                    rect,
                    minimized,
                    maximized,
                    parked,
                    space: Self::desktop_of(l, client),
                    snap,
                    focus: focus == Some(client),
                });
            };
            if l.desktop.enabled {
                for w in &l.desktop.windows {
                    add(w.client, w.rect, w.minimized, w.maximized, w.warped, w.snap);
                }
            } else {
                for f in l.floats() {
                    add(f.client, f.rect, false, false, false, None);
                }
            }
        }
        Snapshot { t: crate::host::wall_now() as u64, style, desktops, cur, wins }
    }

    /// Put `w`'s state on client `c`: rect, parked, minimized, maximized, snap.
    fn history_apply_win(&mut self, w: &Win, c: ClientId) {
        let state = self.state_mut();
        let Some(i) = state.screens.screen_of(c) else { return };
        let l = &mut state.screens.screens[i].layout;
        if l.desktop.enabled {
            if let Some(d) = l.desktop.get_mut(c) {
                d.rect = w.rect;
                d.minimized = w.minimized;
                d.maximized = w.maximized;
                d.snap = w.snap;
                d.warped = w.parked;
            }
        } else if l.is_float(c) {
            l.set_float_rect(c, w.rect);
        }
    }

    /// Put the windows of `snap` that still run where it had them (only
    /// those of desktop `only`, when given). Windows closed since are
    /// skipped; windows opened since stay where they are.
    fn history_apply(&mut self, snap: &Snapshot, only: Option<usize>) {
        let live = self.history_live();
        let who = resolve(&snap.wins, &live);
        for (w, c) in snap.wins.iter().zip(&who) {
            let Some(c) = *c else { continue };
            if only.is_some_and(|o| o != w.space) {
                continue;
            }
            self.history_apply_win(w, c);
        }
        let state = self.state_mut();
        // Back to front, so the relative stacking is the snapshot's.
        for (w, c) in snap.wins.iter().zip(&who) {
            let Some(c) = *c else { continue };
            if only.is_some_and(|o| o != w.space) {
                continue;
            }
            let Some(i) = state.screens.screen_of(c) else { continue };
            let l = &mut state.screens.screens[i].layout;
            l.raise_float(c);
            if w.focus && !w.minimized {
                l.set_focus(c);
            }
        }
    }

    /// At startup: make the desktops of the last run again and wait for its
    /// windows to come back.
    pub(crate) fn history_restore(&mut self) {
        let style = self.state_mut().style.target as u8;
        let Some(snap) = self.state_mut().history.ring.snaps.iter().rev().find(|s| s.style == style).cloned() else {
            return;
        };
        let state = self.state_mut();
        for s in &mut state.screens.screens {
            while s.layout.spaces.desktop_count() < snap.desktops {
                if s.layout.new_desktop().is_none() {
                    break;
                }
            }
        }
        let h = &mut state.history;
        h.pending = snap.wins.into_iter().map(|w| Win { client: None, ..w }).collect();
        h.pending_until = crate::host::now() + RESTORE_SECS;
    }

    /// A window that came back goes to its desktop, in its old state.
    fn history_place(&mut self, cx: &mut Cx) {
        if self.state_mut().history.pending.is_empty() {
            return;
        }
        if crate::host::now() > self.state_mut().history.pending_until {
            self.state_mut().history.pending.clear();
            return;
        }
        let placed = self.state_mut().history.placed.clone();
        let live: Vec<Live> = self.history_live().into_iter().filter(|l| !placed.contains(&l.client)).collect();
        let pending = self.state_mut().history.pending.clone();
        let who = resolve(&pending, &live);
        let mut done: Vec<usize> = Vec::new();
        for (k, (w, c)) in pending.iter().zip(&who).enumerate() {
            let Some(c) = *c else { continue };
            self.state_mut().history.placed.push(c);
            done.push(k);
            let Some(screen) = self.state_mut().screens.screen_of(c) else { continue };
            let area = self.screen_area(cx, screen);
            let gap = self.state_mut().gap;
            let layout = self.state_mut().layout_at_mut(screen);
            if let Some(idx) = layout.spaces.desktop_index(w.space) {
                layout.move_client_to_space(c, idx, area, gap);
            }
            self.history_apply_win(w, c);
        }
        for k in done.into_iter().rev() {
            self.state_mut().history.pending.remove(k);
        }
    }

    /// Look at the arrangement: a change starts the settle timer.
    pub(crate) fn history_poke(&mut self, cx: &mut Cx) {
        if self.state.is_none() || self.drag.is_some() || self.div_drag.is_some() {
            return;
        }
        if self.state_mut().history.scrub.is_some() {
            return;
        }
        self.history_place(cx);
        let snap = self.history_capture();
        let h = &mut self.state_mut().history;
        if h.ring.snaps.last().is_some_and(|l| l.same(&snap)) {
            h.seen = None;
            return;
        }
        if h.seen.as_ref().is_some_and(|s| s.same(&snap)) {
            return;
        }
        h.seen = Some(snap);
        cx.stop_timer(h.settle);
        h.settle = cx.start_timeout(SETTLE);
    }

    fn history_settled(&mut self, cx: &mut Cx) {
        let seen = self.state_mut().history.seen.take();
        if self.drag.is_some() || self.div_drag.is_some() || self.state_mut().history.scrub.is_some() {
            self.history_poke(cx);
            return;
        }
        let snap = self.history_capture();
        if seen.is_some_and(|s| s.same(&snap)) {
            self.state_mut().history.store(snap);
        } else {
            self.history_poke(cx);
        }
    }

    /// Timers, button-ups, key-ups and the keys that end a scrub. True when
    /// the event is consumed.
    pub(crate) fn history_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        if self.state.is_none() {
            return false;
        }
        match event {
            Event::Timer(te) => {
                let (settle, resume) = {
                    let h = &self.state_mut().history;
                    (h.settle.is_timer(te).is_some(), h.resume.is_timer(te).is_some())
                };
                if settle {
                    self.history_settled(cx);
                }
                if resume {
                    // The hold is over: the desk now tells the clients.
                    self.redraw_all(cx);
                }
            }
            Event::MouseUp(_) => self.history_poke(cx),
            Event::KeyDown(e) if self.state_mut().history.scrub.is_some() => {
                if e.key_code == KeyCode::Escape {
                    self.history_finish(cx, false);
                    return true;
                }
                if !crate::super_chord(&e.modifiers) {
                    self.history_finish(cx, true);
                }
            }
            Event::KeyUp(e) => {
                if self.state_mut().history.scrub.is_some() && !crate::super_chord(&e.modifiers) {
                    self.history_finish(cx, true);
                } else {
                    self.history_poke(cx);
                }
            }
            Event::WindowLostFocus(_) if self.state_mut().history.scrub.is_some() => {
                self.history_finish(cx, false);
            }
            _ => {}
        }
        false
    }

    /// SUPER + wheel at `abs`. True when it scrubbed (or is scrubbing), false
    /// when there is nothing to scrub or the pointer is over a window.
    pub(crate) fn history_scroll(&mut self, cx: &mut Cx, dy: f64, abs: Vec2d) -> bool {
        if self.state_mut().style.target.mobile() {
            return false;
        }
        if self.state_mut().history.scrub.is_none() {
            if !self.history_over_background(cx, abs) || !self.history_begin() {
                return false;
            }
        }
        let now = crate::host::now();
        let h = &mut self.state_mut().history;
        let Some(s) = h.scrub.as_mut() else { return false };
        s.acc += dy;
        let steps = (s.acc / STEP).trunc();
        if steps == 0.0 {
            return true;
        }
        s.acc -= steps * STEP;
        // Wheel down goes back in time.
        let to = if steps > 0.0 {
            s.cursor.saturating_sub(steps as usize)
        } else {
            (s.cursor + (-steps) as usize).min(s.start)
        };
        if to == s.cursor {
            return true;
        }
        s.cursor = to;
        let (at, n, base) = (s.list.get(to).copied(), s.list.len(), s.base.clone());
        let (snap, label) = match at.filter(|_| to != s.start) {
            None => (base, "Current layout".to_string()),
            Some(i) => {
                let snap = h.ring.snaps[i].clone();
                let age = (crate::host::wall_now() as u64).saturating_sub(snap.t);
                (snap, format!("Layout {} ({}/{})", ago(age), to + 1, n))
            }
        };
        h.hold_until = now + RESIZE_DELAY;
        cx.stop_timer(h.resume);
        h.resume = cx.start_timeout(RESIZE_DELAY + 0.02);
        let only = self.state_mut().history.scrub.as_ref().map(|s| s.space);
        self.history_apply(&snap, only);
        self.show_osd(cx, OsdShow { icon: Ico::Refresh, value: None, message: label, duration: OSD_SECS });
        self.redraw_all(cx);
        true
    }

    /// Nothing but the desktop under `abs`: no window, parked or not.
    fn history_over_background(&mut self, cx: &mut Cx, abs: Vec2d) -> bool {
        if self.warp_window_at(cx, abs).is_some() {
            return false;
        }
        let screen = self.state_mut().screen_under(abs.x, abs.y);
        let area = self.screen_area(cx, screen);
        let state = self.state_mut();
        if state.span_at(abs.x, abs.y).is_some() {
            return false;
        }
        let gap = state.gap;
        state.layout_at(screen).client_at(abs.x, abs.y, area, gap).is_none()
    }

    /// Start a scrub; false when there is nowhere to go.
    fn history_begin(&mut self) -> bool {
        let space = self.history_space();
        let base = self.history_capture();
        let h = &mut self.state_mut().history;
        let list = h.ring.of_space(base.style, space);
        let start = match list.last() {
            Some(&i) if h.ring.snaps[i].same_in(&base, space) => list.len() - 1,
            _ => list.len(),
        };
        if start == 0 {
            return false;
        }
        h.seen = None;
        h.scrub = Some(Scrub { base, list, space, start, cursor: start, acc: 0.0 });
        true
    }

    /// End the scrub. `keep`: the shown layout stays (the next settle stores
    /// it); otherwise the layout from before comes back.
    fn history_finish(&mut self, cx: &mut Cx, keep: bool) {
        let Some(s) = self.state_mut().history.scrub.take() else { return };
        let only = Some(s.space);
        if !keep {
            self.history_apply(&s.base, only);
        }
        // Focus follows the layout, then the stacking is put back as shown.
        self.focus_after_layout(cx);
        let shown = if keep && s.cursor != s.start {
            self.state_mut().history.ring.snaps[s.list[s.cursor]].clone()
        } else {
            s.base
        };
        self.history_apply(&shown, only);
        let osd = self.ui.widget(cx, ids!(shell_osd));
        if let Some(o) = osd.borrow_mut::<crate::shell::osd::ShellOsd>().as_mut() {
            o.hide(cx);
        }
        self.history_poke(cx);
        self.redraw_all(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::store::{parse, Snapshot, Win};
    use super::History;
    use crate::layout::LRect;

    fn snap(t: u64, app: &str, x: f64) -> Snapshot {
        let w = Win { app: app.into(), title: String::new(), client: None, rect: LRect::new(x, 0.0, 100.0, 100.0),
            minimized: false, maximized: false, parked: false, space: 0, snap: None, focus: false };
        Snapshot { t, style: 1, desktops: 1, cur: 0, wins: vec![w] }
    }

    /// Somebody edits the file (takes windows out) while the WM runs: the WM's next
    /// write keeps the edit instead of writing its old memory back over it.
    #[test]
    fn an_edit_of_the_file_is_kept_by_the_next_write() {
        let dir = std::env::temp_dir().join(format!("wm-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("layouts");
        let mut h = History::default();
        h.store_at(&path, snap(1, "notes", 0.0));
        h.store_at(&path, snap(2, "code", 0.0));
        assert_eq!(parse(&std::fs::read_to_string(&path).unwrap()).snaps.len(), 2);
        // The edit: only the notes layout stays (written a moment later, a new mtime).
        std::thread::sleep(std::time::Duration::from_millis(20));
        let edited = super::store::serialise(&{
            let mut r = super::store::Ring::default();
            r.push(snap(1, "notes", 0.0));
            r
        });
        std::fs::write(&path, edited).unwrap();
        h.store_at(&path, snap(3, "notes", 50.0));
        let back = parse(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(back.snaps.iter().map(|s| s.t).collect::<Vec<_>>(), vec![1, 3], "the code layout the edit removed came back");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
