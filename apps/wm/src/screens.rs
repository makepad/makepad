//! Pure screen geometry for per-screen layouts on a wide Linux desktop.
//!
//! Nothing here touches a `Cx` or any platform type: every function takes
//! plain `LRect`s and primitives, so it unit-tests without a display and
//! without the GPU. The gate that decides whether per-screen behaviour runs
//! at all (`per_screen_enabled`) is a pure function for the same reason —
//! see `docs/research/2026-10-02-wm-per-screen-map.md` §4.
//!
//! Coordinate space: the same window-coordinate space as `MouseEvent.abs`
//! and the WM's own `LRect` (see `layout.rs`). On Linux direct this is the
//! wide desktop's window coordinates; callers must not feed this module
//! macOS/Windows `screens()` output, which is in OS display coordinates.

use std::collections::HashMap;

use crate::layout::{transfer_client, ClientId, Detached, LRect, WmLayout, SCRATCHPAD};

/// How long (seconds) a screen name must stay missing before its windows
/// migrate (map §6.3 rule 5): mode changes and `active=false` flaps must
/// not scatter windows.
pub const REMOVAL_DEBOUNCE: f64 = 2.0;

/// Running the Linux direct backend and not the gallery. Otherwise
/// per-screen behaviour is off and the WM uses one fallback entry covering
/// the whole desk (today's behaviour). A mobile style does NOT turn it off:
/// the per-screen layouts are kept and only the active one is shown. How
/// many screens it takes is `screen_rects_for`'s business.
pub fn per_screen_enabled(linux_direct: bool, gallery: bool) -> bool {
    linux_direct && !gallery
}

/// The intersection of `screen` and `desk`; `None` if they don't overlap
/// (or touch only along an edge, which has zero area).
pub fn clip_to_desk(screen: LRect, desk: LRect) -> Option<LRect> {
    let x0 = screen.x.max(desk.x);
    let y0 = screen.y.max(desk.y);
    let x1 = (screen.x + screen.w).min(desk.x + desk.w);
    let y1 = (screen.y + screen.h).min(desk.y + desk.h);
    if x1 > x0 && y1 > y0 {
        Some(LRect::new(x0, y0, x1 - x0, y1 - y0))
    } else {
        None
    }
}

/// The per-screen rects the desk should use, left to right: each screen
/// that survives clipping is `(name, clip_to_desk(geom, desk))`, in the
/// same order as `names`/`geoms` (a screen fully off the desk is skipped).
///
/// - `per_screen` false (gallery, not Linux direct: macOS/Windows
///   `screens()` aren't desk coordinates): one fallback entry `("", desk)`.
/// - `named` false (the set is still the `""` fallback, see
///   `ScreenSet::is_named`): it takes at least two clipped screens to go
///   per-screen; fewer, or `names`/`geoms` disagreeing in length, gives the
///   fallback — today's single-screen behaviour.
/// - `named` true: a named set never falls back on a hotplug. One clipped
///   screen stays a named entry, so a lost screen goes through the removal
///   debounce; a length mismatch (names and geoms read mid-hotplug) or no
///   clipped screen at all gives an EMPTY Vec, which `ScreenSet::reconcile`
///   ignores (wait for the next publish).
pub fn screen_rects_for(
    per_screen: bool,
    named: bool,
    names: &[String],
    geoms: &[LRect],
    desk: LRect,
) -> Vec<(String, LRect)> {
    let fallback = || vec![("".to_string(), desk)];
    if !per_screen {
        return fallback();
    }
    if names.len() != geoms.len() {
        return if named { Vec::new() } else { fallback() };
    }
    let clipped: Vec<(String, LRect)> = names
        .iter()
        .zip(geoms.iter())
        .filter_map(|(name, geom)| clip_to_desk(*geom, desk).map(|r| (name.clone(), r)))
        .collect();
    let need = if named { 1 } else { 2 };
    if clipped.len() >= need {
        clipped
    } else if named {
        Vec::new()
    } else {
        fallback()
    }
}

/// Squared Euclidean distance from `(x, y)` to the nearest point of `r`
/// (zero when the point is inside or on its boundary).
fn dist_sq_to_rect(x: f64, y: f64, r: &LRect) -> f64 {
    let dx = if x < r.x {
        r.x - x
    } else if x > r.x + r.w {
        x - (r.x + r.w)
    } else {
        0.0
    };
    let dy = if y < r.y {
        r.y - y
    } else if y > r.y + r.h {
        y - (r.y + r.h)
    } else {
        0.0
    };
    dx * dx + dy * dy
}

/// Which screen a point belongs to. Edges are inclusive, so a point on a
/// seam shared by two screens belongs to the first of them in `rects`
/// order. Outside every screen (e.g. the dead band under a shorter screen
/// on a wide desktop), the nearest screen by Euclidean distance wins.
/// `None` only when `rects` is empty.
pub fn screen_at(rects: &[LRect], x: f64, y: f64) -> Option<usize> {
    if rects.is_empty() {
        return None;
    }
    for (i, r) in rects.iter().enumerate() {
        if x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h {
            return Some(i);
        }
    }
    rects
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            dist_sq_to_rect(x, y, a)
                .partial_cmp(&dist_sq_to_rect(x, y, b))
                .unwrap()
        })
        .map(|(i, _)| i)
}

/// A screen's usable tiling area: `rect` with `reserved_bottom` (the dock,
/// per that screen) removed from its bottom edge, then inset by
/// `gaps_out` on every side.
pub fn screen_area(rect: LRect, reserved_bottom: f64, gaps_out: f64) -> LRect {
    let h = (rect.h - reserved_bottom).max(0.0);
    let w = (rect.w - gaps_out * 2.0).max(0.0);
    let h = (h - gaps_out * 2.0).max(0.0);
    LRect::new(rect.x + gaps_out, rect.y + gaps_out, w, h)
}

/// One physical screen and the layout that tiles it.
pub struct ScreenLayout {
    /// Connector name (`""` for the single fallback entry).
    pub name: String,
    /// Desk-clipped screen rect, before gaps and the dock reservation.
    pub rect: LRect,
    pub layout: WmLayout,
}

/// One `WmLayout` per screen, left to right in `screens()` order.
///
/// Index invariants (after every `reconcile`):
/// - `screens[..live_count()]` are the LIVE screens, index-aligned with the
///   last non-empty `reconcile` input.
/// - `screens[live_count()..]` are PENDING: missing from the input, waiting
///   out `REMOVAL_DEBOUNCE` with their layouts intact, at stale rects. They
///   are never drawn, never hit by the pointer, never a move-to-screen
///   source or target, and never made active.
/// - `active < live_count()` and `main < live_count()`.
pub struct ScreenSet {
    pub screens: Vec<ScreenLayout>,
    /// The pointer / explicit-focus screen: new windows, menus and
    /// workspace keys act here. Always a live screen.
    pub active: usize,
    /// The primary screen, from `main_name`, else 0. The dock sits here, so
    /// `reserved_bottom` (the dock's height) applies to this screen only.
    pub main: usize,
    /// In-session: the connector a migrated window came from, so it goes
    /// back when that screen returns.
    pub homes: HashMap<ClientId, String>,
    /// Screen name and the time it was first seen missing.
    pending_removal: Vec<(String, f64)>,
    /// The live screen the pointer was last seen on (`on_pointer`), so a
    /// crossing is a change of the POINTER's screen: a focus change that
    /// made another screen active is not undone by the next mouse move.
    /// Reset by `reconcile`, which may shift indices.
    pointer: Option<usize>,
}

fn contains_rect(outer: LRect, inner: LRect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && outer.x + outer.w >= inner.x + inner.w
        && outer.y + outer.h >= inner.y + inner.h
}

/// How far a window moves when it goes from screen `src` to `dst`: not at
/// all when `dst` covers `src` (a merge into the whole desk keeps
/// positions), otherwise by the origin delta (same place on the new screen).
fn carry_offset(src: LRect, dst: LRect) -> (f64, f64) {
    if contains_rect(dst, src) {
        (0.0, 0.0)
    } else {
        (dst.x - src.x, dst.y - src.y)
    }
}

/// `a` and `b` (distinct) of one slice, both mutably.
fn two_mut<T>(v: &mut [T], a: usize, b: usize) -> (&mut T, &mut T) {
    assert_ne!(a, b);
    if a < b {
        let (l, r) = v.split_at_mut(b);
        (&mut l[a], &mut r[0])
    } else {
        let (l, r) = v.split_at_mut(a);
        (&mut r[0], &mut l[b])
    }
}

/// Put a client detached from screen `src_rect` on `dst`'s workspace `ws`;
/// `adopt` shifts its float and desktop-style rects by the screen offset and
/// fits them inside `area`.
fn place(src_rect: LRect, dst: &mut ScreenLayout, ws: usize, d: Detached, area: LRect, gap: f64) {
    let off = carry_offset(src_rect, dst.rect);
    dst.layout.adopt(ws, d, off, area, gap);
}

impl ScreenSet {
    /// Today's single-desk WM: one fallback entry named `""` over `desk`.
    pub fn new(layout: WmLayout, desk: LRect) -> Self {
        Self {
            screens: vec![ScreenLayout { name: String::new(), rect: desk, layout }],
            active: 0,
            main: 0,
            homes: HashMap::new(),
            pending_removal: Vec::new(),
            pointer: None,
        }
    }

    pub fn active_layout(&self) -> &WmLayout {
        &self.screens[self.active].layout
    }

    pub fn active_layout_mut(&mut self) -> &mut WmLayout {
        &mut self.screens[self.active].layout
    }

    /// Which screen's layout holds `c`.
    pub fn screen_of(&self, c: ClientId) -> Option<usize> {
        self.screens.iter().position(|s| s.layout.workspace_of(c).is_some())
    }

    /// The active screen's focused client.
    pub fn focused_client(&self) -> Option<ClientId> {
        self.active_layout().focused_client()
    }

    pub fn all_clients(&self) -> Vec<ClientId> {
        self.screens.iter().flat_map(|s| s.layout.all_clients()).collect()
    }

    /// Every screen's rect, index-aligned with `screens` (a screen waiting
    /// out the removal debounce included, at the end).
    pub fn rects(&self) -> Vec<LRect> {
        self.screens.iter().map(|s| s.rect).collect()
    }

    /// Not the single `""` fallback any more (see `screen_rects_for`'s
    /// `named`).
    pub fn is_named(&self) -> bool {
        !(self.screens.len() == 1 && self.screens[0].name.is_empty())
    }

    /// How many screens are live; they are `screens[..live_count()]`, and
    /// the pending ones follow. Multi-screen behaviour (per-screen bar,
    /// move-to-screen, fullscreen-as-maximize) is `live_count() >= 2`.
    pub fn live_count(&self) -> usize {
        self.screens.len() - self.pending_removal.len()
    }

    /// Make the live screen holding `c` active (`focus_client` on another
    /// screen's window). `None`, and `active` unchanged, when `c` is
    /// unknown or sits on a pending screen.
    pub fn activate_screen_of(&mut self, c: ClientId) -> Option<usize> {
        let i = self.screen_of(c).filter(|&i| i < self.live_count())?;
        self.active = i;
        Some(i)
    }

    /// Where a shell surface (menu, flyout, OSD, notifications) opened for
    /// live screen `i` draws: that screen's rect, or the active screen's
    /// when `i` is no longer live. `None` with fewer than two live screens:
    /// the surface keeps today's whole-overlay rect.
    pub fn surface_rect(&self, i: usize) -> Option<LRect> {
        let live = self.live_count();
        if live < 2 {
            return None;
        }
        let i = if i < live { i } else { self.active };
        self.screens.get(i).map(|s| s.rect)
    }

    /// The windows every live screen shows (its visible workspace's),
    /// in screen order, each screen's in `clients_on` order: what the one
    /// dock or taskbar lists, the same whichever screen is active. One
    /// screen gives exactly its `clients_on(active)`.
    pub fn shown_clients(&self) -> Vec<ClientId> {
        self.screens[..self.live_count()]
            .iter()
            .flat_map(|s| s.layout.clients_on(s.layout.active))
            .collect()
    }

    /// The main screen's rect, where the dock sits; `None` with fewer than
    /// two live screens (the dock spans the whole overlay, as before).
    pub fn main_rect(&self) -> Option<LRect> {
        if self.live_count() < 2 || self.main >= self.live_count() {
            return None;
        }
        Some(self.screens[self.main].rect)
    }

    /// The dock's reservation on screen `i`: the dock sits on the main
    /// screen only, so every other screen keeps its full height.
    pub fn reserved_for(&self, i: usize, reserved_bottom: f64) -> f64 {
        if i == self.main {
            reserved_bottom
        } else {
            0.0
        }
    }

    /// Screen `i`'s tiling area (`screen_area` with the dock reserved only
    /// on the main screen), never narrower or shorter than one point, like
    /// the WM's single-desk area.
    pub fn area(&self, i: usize, reserved_bottom: f64, gaps_out: f64) -> LRect {
        let a = screen_area(self.screens[i].rect, self.reserved_for(i, reserved_bottom), gaps_out);
        LRect::new(a.x, a.y, a.w.max(1.0), a.h.max(1.0))
    }

    /// Indices of the screens that are really there (not waiting out the
    /// removal debounce), left to right.
    fn live(&self) -> Vec<usize> {
        (0..self.live_count()).collect()
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        self.screens.iter().position(|s| s.name == name)
    }

    /// A kept screen got `rect`: shift its floats by the move, keep them
    /// reachable, and hand the layout its new outer rect.
    fn set_rect(s: &mut ScreenLayout, rect: LRect, reserved_bottom: f64, gaps_out: f64) {
        if s.rect != rect {
            // A screen that grew into, or shrank to part of, its old rect
            // (the "" desk renamed to a screen, a screen merged into the
            // desk) keeps its windows where they are; only a moved screen
            // carries them along.
            let (dx, dy) = if contains_rect(rect, s.rect) || contains_rect(s.rect, rect) {
                (0.0, 0.0)
            } else {
                (rect.x - s.rect.x, rect.y - s.rect.y)
            };
            s.layout.translate(dx, dy);
            s.layout.fit_floats(screen_area(rect, reserved_bottom, gaps_out));
        }
        s.layout.set_outer(rect);
        s.rect = rect;
    }

    /// Every client of screen `src` goes to `dst` on the same workspace
    /// number (scratchpad included), tiles re-inserted in focus order, each
    /// recorded in `homes` (an earlier home wins). `dst` keeps the focus it
    /// had on a workspace that already had one.
    fn migrate_all(&mut self, src: usize, dst: usize, gap: f64, reserved_bottom: f64, gaps_out: f64) {
        let src_name = self.screens[src].name.clone();
        let rb = self.reserved_for(dst, reserved_bottom);
        let (s, d) = two_mut(&mut self.screens, src, dst);
        let area = screen_area(d.rect, rb, gaps_out);
        for ws in 0..=SCRATCHPAD {
            let taken = s.layout.take_workspace_clients(ws);
            if taken.is_empty() {
                continue;
            }
            let prior = d.layout.workspaces[ws].focus;
            for t in taken {
                let c = t.client;
                place(s.rect, d, ws, t, area, gap);
                if !src_name.is_empty() {
                    self.homes.entry(c).or_insert_with(|| src_name.clone());
                }
            }
            if prior.is_some() {
                d.layout.workspaces[ws].focus = prior;
            }
        }
    }

    /// Clients whose home is screen `dst` go back to it from wherever they
    /// are now (same workspace number) and leave `homes`.
    fn return_home(&mut self, dst: usize, gap: f64, reserved_bottom: f64, gaps_out: f64) {
        let name = self.screens[dst].name.clone();
        let rb = self.reserved_for(dst, reserved_bottom);
        let mut back: Vec<ClientId> =
            self.homes.iter().filter(|(_, h)| **h == name).map(|(c, _)| *c).collect();
        back.sort();
        for c in back {
            self.homes.remove(&c);
            let Some(src) = self.screen_of(c) else { continue };
            if src == dst {
                continue;
            }
            let (s, d) = two_mut(&mut self.screens, src, dst);
            let Some(t) = s.layout.detach(c) else { continue };
            let area = screen_area(d.rect, rb, gaps_out);
            place(s.rect, d, t.ws, t, area, gap);
        }
    }

    /// Bring the set in line with the screens now published (`new`, left to
    /// right, from `screen_rects_for`). `now` is in seconds. See map §6.3:
    /// - same name: layout kept, floats moved with the screen and fitted;
    /// - the old single `""` entry is renamed to `main_name` (else the
    ///   first new name), never migrated;
    /// - back to the single `""` entry: every other screen merges into the
    ///   survivor (the main screen) at once, same workspace numbers;
    /// - added name: an empty layout, then its `homes` clients come back;
    /// - removed name: kept (at the end) until missing for
    ///   `REMOVAL_DEBOUNCE`, then migrated to the main screen (else the
    ///   active one) on the same workspace numbers, recorded in `homes`.
    /// `active` stays on its screen by name (the main screen if that one
    /// went), `main` is `main_name`'s index, else 0. An empty `new` is
    /// ignored. A merged survivor's own clients have no `homes` entry, so
    /// after a later split they stay on the screen the fallback is renamed to.
    pub fn reconcile(
        &mut self,
        new: &[(String, LRect)],
        main_name: Option<&str>,
        now: f64,
        gap: f64,
        reserved_bottom: f64,
        gaps_out: f64,
    ) {
        if new.is_empty() {
            return;
        }
        self.pointer = None;
        let main_name = main_name.filter(|m| new.iter().any(|(n, _)| n == m));
        let active_name = self.screens.get(self.active).map(|s| s.name.clone());

        if new.len() == 1 && new[0].0.is_empty() {
            // Back to one desk: merge everything into the survivor.
            let keep = self
                .index_of("")
                .unwrap_or_else(|| self.main.min(self.screens.len() - 1));
            // The survivor is the one desk, so the dock is on it.
            self.main = keep;
            Self::set_rect(&mut self.screens[keep], new[0].1, reserved_bottom, gaps_out);
            for i in (0..self.screens.len()).rev() {
                if i != keep {
                    self.migrate_all(i, keep, gap, reserved_bottom, gaps_out);
                }
            }
            let survivor = self.screens.swap_remove(keep);
            self.screens.clear();
            self.screens.push(ScreenLayout { name: String::new(), ..survivor });
            self.pending_removal.clear();
            self.active = 0;
            self.main = 0;
            self.prune_homes();
            return;
        }

        if self.screens.len() == 1 && self.screens[0].name.is_empty() {
            // The fallback becomes the main screen; windows stay.
            self.screens[0].name = main_name.unwrap_or(&new[0].0).to_string();
        }

        // The dock (`reserved_bottom`) is on the main screen only.
        let main_screen = main_name.unwrap_or(&new[0].0).to_string();
        let mut old = std::mem::take(&mut self.screens);
        let mut added = Vec::new();
        for (name, rect) in new {
            if let Some(i) = old.iter().position(|s| &s.name == name) {
                let mut s = old.remove(i);
                let rb = if *name == main_screen { reserved_bottom } else { 0.0 };
                Self::set_rect(&mut s, *rect, rb, gaps_out);
                self.screens.push(s);
            } else {
                let mut layout = WmLayout::new();
                layout.set_outer(*rect);
                self.screens.push(ScreenLayout { name: name.clone(), rect: *rect, layout });
                added.push(self.screens.len() - 1);
            }
        }
        // What is left in `old` is missing: start (or keep) its debounce.
        self.pending_removal.retain(|(n, _)| old.iter().any(|s| &s.name == n));
        for s in &old {
            if !self.pending_removal.iter().any(|(n, _)| *n == s.name) {
                self.pending_removal.push((s.name.clone(), now));
            }
        }
        self.screens.extend(old);

        self.main = main_name.and_then(|m| self.index_of(m)).unwrap_or(0);
        self.active = active_name
            .and_then(|n| self.index_of(&n))
            .filter(|&i| i < new.len())
            .unwrap_or(self.main);

        let expired: Vec<String> = self
            .pending_removal
            .iter()
            .filter(|(_, t)| now - *t >= REMOVAL_DEBOUNCE)
            .map(|(n, _)| n.clone())
            .collect();
        for name in expired {
            self.pending_removal.retain(|(n, _)| *n != name);
            let Some(src) = self.index_of(&name) else { continue };
            // Pending screens sit after every live one, so `dst < src`.
            let dst = if main_name.is_some() { self.main } else { self.active };
            self.migrate_all(src, dst, gap, reserved_bottom, gaps_out);
            self.screens.remove(src);
        }

        for dst in added {
            self.return_home(dst, gap, reserved_bottom, gaps_out);
        }
        self.prune_homes();
        if self.active >= self.screens.len() {
            self.active = self.main.min(self.screens.len() - 1);
        }
    }

    /// Forget homes of windows that closed.
    fn prune_homes(&mut self) {
        let all = self.all_clients();
        self.homes.retain(|c, _| all.contains(c));
    }

    /// The pointer is at (x, y): `Some(screen)` when the pointer crossed
    /// into a different live screen than the one it was last seen on and
    /// that screen was not already active (it becomes active), `None`
    /// otherwise. A screen made active some other way (a click on the
    /// dock, a bar entry, move-to-screen) stays active until the pointer
    /// itself crosses.
    pub fn on_pointer(&mut self, x: f64, y: f64) -> Option<usize> {
        let live = self.live();
        let rects: Vec<LRect> = live.iter().map(|&i| self.screens[i].rect).collect();
        let i = live[screen_at(&rects, x, y)?];
        let crossed = self.pointer != Some(i);
        self.pointer = Some(i);
        if !crossed || i == self.active {
            return None;
        }
        self.active = i;
        Some(i)
    }

    /// The desk's clip moved (the AI pane, a style's bar) but the physical
    /// screens did not: the live screens, by `new`'s names in the same
    /// order, take their new clipped rects and outers in place. No window
    /// moves or is fitted, so a pane that opens and closes again leaves
    /// every float where it was. False (nothing changed) when `new` does
    /// not name exactly the live screens in order; reconcile then.
    pub fn set_clip(&mut self, new: &[(String, LRect)]) -> bool {
        let live = self.live_count();
        if new.len() != live || new.iter().zip(&self.screens).any(|((n, _), s)| *n != s.name) {
            return false;
        }
        for ((_, rect), s) in new.iter().zip(self.screens.iter_mut()) {
            s.rect = *rect;
            s.layout.set_outer(*rect);
        }
        true
    }

    /// A drag dropped `c` (a float or desktop-style window) over live
    /// screen `to`: it moves there where it already is (no offset, only
    /// fitted into `to`'s area), onto `to`'s active workspace, and `to`
    /// becomes active. False when `c` is unknown, already on `to`, or on a
    /// pending screen.
    pub fn drop_on_screen(
        &mut self,
        c: ClientId,
        to: usize,
        gap: f64,
        reserved_bottom: f64,
        gaps_out: f64,
    ) -> bool {
        let live = self.live_count();
        let Some(from) = self.screen_of(c) else { return false };
        if to >= live || from >= live || from == to {
            return false;
        }
        let rb = self.reserved_for(to, reserved_bottom);
        let (s, d) = two_mut(&mut self.screens, from, to);
        let area = screen_area(d.rect, rb, gaps_out);
        if !transfer_client(&mut s.layout, &mut d.layout, c, (0.0, 0.0), area, gap) {
            return false;
        }
        self.homes.remove(&c);
        self.active = to;
        true
    }

    /// Move `c` to the next (or previous) live screen, wrapping, onto that
    /// screen's active workspace; the target becomes active and `c` its
    /// focus. `None` with one screen or when `c` is unknown.
    pub fn move_to_screen(
        &mut self,
        c: ClientId,
        forward: bool,
        gap: f64,
        reserved_bottom: f64,
        gaps_out: f64,
    ) -> Option<usize> {
        let live = self.live();
        if live.len() < 2 {
            return None;
        }
        let from = self.screen_of(c)?;
        let pos = live.iter().position(|&i| i == from)?;
        let n = live.len();
        let to = live[if forward { (pos + 1) % n } else { (pos + n - 1) % n }];
        if to == from {
            return None;
        }
        let rb = self.reserved_for(to, reserved_bottom);
        let (s, d) = two_mut(&mut self.screens, from, to);
        let offset = (d.rect.x - s.rect.x, d.rect.y - s.rect.y);
        let area = screen_area(d.rect, rb, gaps_out);
        if !transfer_client(&mut s.layout, &mut d.layout, c, offset, area, gap) {
            return None;
        }
        self.homes.remove(&c);
        self.active = to;
        Some(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> String {
        s.to_string()
    }

    // --- per_screen_enabled ---------------------------------------------

    #[test]
    fn per_screen_enabled_needs_linux_direct() {
        assert!(!per_screen_enabled(false, false));
    }

    #[test]
    fn per_screen_enabled_off_for_gallery() {
        assert!(!per_screen_enabled(true, true));
    }

    #[test]
    fn per_screen_enabled_true_when_all_hold() {
        assert!(per_screen_enabled(true, false));
    }

    #[test]
    fn mobile_does_not_affect_the_gate() {
        // The signature has no mobile input: a mobile style keeps the
        // per-screen layouts (only the active one is shown), so a style
        // switch can't send the fallback and merge every screen.
        let direct_desktop = per_screen_enabled(true, false);
        let direct_mobile = per_screen_enabled(true, false);
        assert_eq!(direct_desktop, direct_mobile);
        let desk = LRect::new(0.0, 0.0, 3840.0, 1080.0);
        let names = vec![n("A"), n("B")];
        let geoms = vec![
            LRect::new(0.0, 0.0, 1920.0, 1080.0),
            LRect::new(1920.0, 0.0, 1920.0, 1080.0),
        ];
        assert_eq!(screen_rects_for(direct_mobile, true, &names, &geoms, desk).len(), 2);
    }

    // --- screen_rects_for: fallback cases --------------------------------

    #[test]
    fn screen_rects_for_off_falls_back_to_desk() {
        let desk = LRect::new(0.0, 0.0, 3840.0, 1080.0);
        let names = vec![n("A"), n("B")];
        let geoms = vec![
            LRect::new(0.0, 0.0, 1920.0, 1080.0),
            LRect::new(1920.0, 0.0, 1920.0, 1080.0),
        ];
        let got = screen_rects_for(false, false, &names, &geoms, desk);
        assert_eq!(got, vec![(n(""), desk)]);
    }

    #[test]
    fn screen_rects_for_mismatched_lengths_falls_back_to_desk() {
        let desk = LRect::new(0.0, 0.0, 3840.0, 1080.0);
        let names = vec![n("A"), n("B")];
        let geoms = vec![LRect::new(0.0, 0.0, 1920.0, 1080.0)];
        let got = screen_rects_for(true, false, &names, &geoms, desk);
        assert_eq!(got, vec![(n(""), desk)]);
    }

    #[test]
    fn screen_rects_for_single_surviving_screen_falls_back_to_desk() {
        // One geom entirely off the desk clips to empty, leaving only one
        // non-empty rect -- fewer than two, so it falls back.
        let desk = LRect::new(0.0, 0.0, 1920.0, 1080.0);
        let names = vec![n("A"), n("B")];
        let geoms = vec![
            LRect::new(0.0, 0.0, 1920.0, 1080.0),
            LRect::new(5000.0, 0.0, 1920.0, 1080.0),
        ];
        let got = screen_rects_for(true, false, &names, &geoms, desk);
        assert_eq!(got, vec![(n(""), desk)]);
    }

    #[test]
    fn screen_rects_for_unnamed_single_screen_falls_back() {
        let desk = LRect::new(0.0, 0.0, 1920.0, 1080.0);
        let names = vec![n("A")];
        let geoms = vec![LRect::new(0.0, 0.0, 1920.0, 1080.0)];
        assert_eq!(screen_rects_for(true, false, &names, &geoms, desk), vec![(n(""), desk)]);
    }

    #[test]
    fn screen_rects_for_off_falls_back_even_when_named() {
        let desk = LRect::new(0.0, 0.0, 3840.0, 1080.0);
        let names = vec![n("A"), n("B")];
        let geoms = vec![
            LRect::new(0.0, 0.0, 1920.0, 1080.0),
            LRect::new(1920.0, 0.0, 1920.0, 1080.0),
        ];
        assert_eq!(screen_rects_for(false, true, &names, &geoms, desk), vec![(n(""), desk)]);
    }

    #[test]
    fn screen_rects_for_named_keeps_one_remaining_screen() {
        let desk = LRect::new(0.0, 0.0, 1920.0, 1080.0);
        let names = vec![n("A")];
        let geoms = vec![LRect::new(0.0, 0.0, 1920.0, 1080.0)];
        assert_eq!(screen_rects_for(true, true, &names, &geoms, desk), vec![(n("A"), desk)]);
    }

    #[test]
    fn screen_rects_for_named_mismatch_or_nothing_left_is_empty() {
        let desk = LRect::new(0.0, 0.0, 1920.0, 1080.0);
        let names = vec![n("A"), n("B")];
        let geoms = vec![LRect::new(0.0, 0.0, 1920.0, 1080.0)];
        assert!(screen_rects_for(true, true, &names, &geoms, desk).is_empty());
        let off = vec![LRect::new(5000.0, 0.0, 1920.0, 1080.0)];
        assert!(screen_rects_for(true, true, &[n("A")], &off, desk).is_empty());
        assert!(screen_rects_for(true, true, &[], &[], desk).is_empty());
    }

    // --- screen_rects_for: the two-screen wide-desktop cases -------------

    #[test]
    fn two_equal_screens_clip_below_the_bar() {
        // Bar strip on top: desk starts at y=26.
        let desk = LRect::new(0.0, 26.0, 3840.0, 1080.0 - 26.0);
        let names = vec![n("DP-1"), n("DP-2")];
        let geoms = vec![
            LRect::new(0.0, 0.0, 1920.0, 1080.0),
            LRect::new(1920.0, 0.0, 1920.0, 1080.0),
        ];
        let got = screen_rects_for(true, false, &names, &geoms, desk);
        assert_eq!(
            got,
            vec![
                (n("DP-1"), LRect::new(0.0, 26.0, 1920.0, 1080.0 - 26.0)),
                (n("DP-2"), LRect::new(1920.0, 26.0, 1920.0, 1080.0 - 26.0)),
            ]
        );
    }

    #[test]
    fn shorter_right_screen_has_no_dead_band() {
        let desk = LRect::new(0.0, 0.0, 3840.0, 1080.0);
        let names = vec![n("DP-1"), n("DP-2")];
        let geoms = vec![
            LRect::new(0.0, 0.0, 1920.0, 1080.0),
            LRect::new(1920.0, 0.0, 1920.0, 720.0),
        ];
        let got = screen_rects_for(true, false, &names, &geoms, desk);
        assert_eq!(
            got,
            vec![
                (n("DP-1"), LRect::new(0.0, 0.0, 1920.0, 1080.0)),
                (n("DP-2"), LRect::new(1920.0, 0.0, 1920.0, 720.0)),
            ]
        );
    }

    #[test]
    fn ai_pane_clips_the_left_screen_start() {
        // AI pane open: desk starts at x=400.
        let desk = LRect::new(400.0, 0.0, 3440.0, 1080.0);
        let names = vec![n("DP-1"), n("DP-2")];
        let geoms = vec![
            LRect::new(0.0, 0.0, 1920.0, 1080.0),
            LRect::new(1920.0, 0.0, 1920.0, 1080.0),
        ];
        let got = screen_rects_for(true, false, &names, &geoms, desk);
        assert_eq!(
            got,
            vec![
                (n("DP-1"), LRect::new(400.0, 0.0, 1920.0 - 400.0, 1080.0)),
                (n("DP-2"), LRect::new(1920.0, 0.0, 1920.0, 1080.0)),
            ]
        );
    }

    // --- screen_at --------------------------------------------------------

    #[test]
    fn screen_at_seam_belongs_to_the_first_screen() {
        let rects = vec![
            LRect::new(0.0, 0.0, 1920.0, 1080.0),
            LRect::new(1920.0, 0.0, 1920.0, 720.0),
        ];
        // x = 1920 is the right edge of screen 0 and the left edge of
        // screen 1 -- the first in order wins.
        assert_eq!(screen_at(&rects, 1920.0, 100.0), Some(0));
    }

    #[test]
    fn screen_at_dead_band_picks_the_nearest_screen() {
        let rects = vec![
            LRect::new(0.0, 0.0, 1920.0, 1080.0),
            LRect::new(1920.0, 0.0, 1920.0, 720.0),
        ];
        // Below the shorter right screen's bottom edge, still within its
        // x-range: outside both rects, nearest is screen 1.
        assert_eq!(screen_at(&rects, 2500.0, 900.0), Some(1));
    }

    #[test]
    fn screen_at_empty_is_none() {
        let rects: Vec<LRect> = vec![];
        assert_eq!(screen_at(&rects, 0.0, 0.0), None);
    }

    // --- screen_area --------------------------------------------------------

    #[test]
    fn screen_area_applies_reserved_bottom_and_gaps() {
        let rect = LRect::new(0.0, 26.0, 1920.0, 1054.0);
        let got = screen_area(rect, 60.0, 10.0);
        // Height loses 60 off the bottom, then 10 off every side.
        assert_eq!(
            got,
            LRect::new(10.0, 36.0, 1920.0 - 20.0, 1054.0 - 60.0 - 20.0)
        );
    }
    // --- ScreenSet ----------------------------------------------------------

    const GAP: f64 = 0.0;
    const RB: f64 = 0.0;
    const GO: f64 = 0.0;
    const DESK: LRect = LRect { x: 0.0, y: 0.0, w: 3840.0, h: 1080.0 };
    const RA: LRect = LRect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };
    const RB_: LRect = LRect { x: 1920.0, y: 0.0, w: 1920.0, h: 1080.0 };

    fn ab() -> Vec<(String, LRect)> {
        vec![(n("A"), RA), (n("B"), RB_)]
    }

    fn sorted(mut v: Vec<ClientId>) -> Vec<ClientId> {
        v.sort();
        v
    }

    /// Fallback "" layout: 1, 2 tiled on workspace 0, float 3 on ws 0.
    fn fallback_set() -> ScreenSet {
        let mut l = WmLayout::new();
        let area = screen_area(DESK, RB, GO);
        l.insert(1, area, GAP);
        l.insert(2, area, GAP);
        l.add_float(3, LRect::new(100.0, 100.0, 400.0, 300.0), 0);
        ScreenSet::new(l, DESK)
    }

    /// [A, B] with B holding 10, 11 tiled on ws 0, 12 tiled on ws 3,
    /// float 13 on ws 0 and 14 on the scratchpad; A holds 1 on ws 0.
    fn two_screens() -> ScreenSet {
        let mut set = ScreenSet::new(WmLayout::new(), DESK);
        set.reconcile(&ab(), Some("A"), 0.0, GAP, RB, GO);
        let aa = screen_area(RA, RB, GO);
        let ba = screen_area(RB_, RB, GO);
        set.screens[0].layout.insert(1, aa, GAP);
        let b = &mut set.screens[1].layout;
        b.insert_on(0, 10, ba, GAP);
        b.insert_on(0, 11, ba, GAP);
        b.insert_on(3, 12, ba, GAP);
        b.add_float(13, LRect::new(2000.0, 100.0, 400.0, 300.0), 0);
        b.insert_on(SCRATCHPAD, 14, ba, GAP);
        set
    }

    #[test]
    fn new_is_one_fallback_entry() {
        let set = fallback_set();
        assert_eq!(set.screens.len(), 1);
        assert_eq!(set.screens[0].name, "");
        assert_eq!(set.rects(), vec![DESK]);
        assert_eq!(set.active, 0);
        assert_eq!(sorted(set.all_clients()), vec![1, 2, 3]);
    }

    #[test]
    fn fallback_is_renamed_to_main_and_keeps_its_windows() {
        let mut set = fallback_set();
        set.reconcile(&ab(), Some("A"), 0.0, GAP, RB, GO);
        assert_eq!(set.screens.len(), 2);
        assert_eq!(set.screens[0].name, "A");
        assert_eq!(set.screens[1].name, "B");
        assert_eq!(set.rects(), vec![RA, RB_]);
        assert_eq!(set.main, 0);
        assert_eq!(sorted(set.screens[0].layout.all_clients()), vec![1, 2, 3]);
        assert!(set.screens[1].layout.all_clients().is_empty());
        assert_eq!(set.screen_of(3), Some(0));
        assert!(set.homes.is_empty());
    }

    #[test]
    fn fallback_is_renamed_to_a_main_on_the_right() {
        let mut set = fallback_set();
        set.reconcile(&ab(), Some("B"), 0.0, GAP, RB, GO);
        assert_eq!(set.main, 1);
        // The desk at x=0 became B at x=1920: B lies inside the old desk,
        // so nothing is shifted by +1920; the float on A's half is only
        // fitted to keep its title bar reachable on B.
        let f = set.screens[1].layout.float_rect(3).unwrap();
        assert_eq!((f.w, f.h, f.y), (400.0, 300.0, 100.0));
        assert!(f.x < 1920.0, "float was shifted by the origin delta: {:?}", f);
        assert_eq!(sorted(set.screens[1].layout.all_clients()), vec![1, 2, 3]);
        assert!(set.screens[0].layout.all_clients().is_empty());
    }

    #[test]
    fn fallback_without_main_goes_to_the_first_screen() {
        let mut set = fallback_set();
        set.reconcile(&ab(), None, 0.0, GAP, RB, GO);
        assert_eq!(set.main, 0);
        assert_eq!(sorted(set.screens[0].layout.all_clients()), vec![1, 2, 3]);
    }

    #[test]
    fn removal_waits_for_the_debounce() {
        let mut set = two_screens();
        set.reconcile(&[(n("A"), RA)], Some("A"), 10.0, GAP, RB, GO);
        set.reconcile(&[(n("A"), RA)], Some("A"), 11.9, GAP, RB, GO);
        assert_eq!(set.screens.len(), 2);
        assert_eq!(set.screens[1].name, "B");
        assert_eq!(sorted(set.screens[1].layout.all_clients()), vec![10, 11, 12, 13, 14]);
        assert_eq!(set.screens[0].layout.all_clients(), vec![1]);
        assert!(set.homes.is_empty());
    }

    #[test]
    fn a_flap_shorter_than_the_debounce_changes_nothing() {
        let mut set = two_screens();
        set.reconcile(&[(n("A"), RA)], Some("A"), 10.0, GAP, RB, GO);
        set.reconcile(&ab(), Some("A"), 11.0, GAP, RB, GO);
        assert_eq!(set.screens.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["A", "B"]);
        assert_eq!(set.live_count(), 2);
        assert_eq!(sorted(set.screens[1].layout.all_clients()), vec![10, 11, 12, 13, 14]);
        // Gone again later: the debounce starts over.
        set.reconcile(&[(n("A"), RA)], Some("A"), 12.5, GAP, RB, GO);
        assert_eq!(set.screens.len(), 2);
        assert_eq!(sorted(set.screens[1].layout.all_clients()), vec![10, 11, 12, 13, 14]);
    }

    #[test]
    fn removal_after_the_debounce_migrates_to_main_on_the_same_workspaces() {
        let mut set = two_screens();
        set.reconcile(&[(n("A"), RA)], Some("A"), 10.0, GAP, RB, GO);
        set.reconcile(&[(n("A"), RA)], Some("A"), 12.0, GAP, RB, GO);
        assert_eq!(set.screens.len(), 1);
        assert_eq!(set.screens[0].name, "A");
        let a = &set.screens[0].layout;
        assert_eq!(sorted(a.clients_on(0)), vec![1, 10, 11, 13]);
        assert_eq!(a.clients_on(3), vec![12]);
        assert_eq!(a.clients_on(SCRATCHPAD), vec![14]);
        // The float moved with its screen (B's origin to A's) and stayed whole.
        assert_eq!(a.float_rect(13), Some(LRect::new(80.0, 100.0, 400.0, 300.0)));
        for c in [10, 11, 12, 13, 14] {
            assert_eq!(set.homes.get(&c).map(|s| s.as_str()), Some("B"));
        }
        assert!(set.homes.get(&1).is_none());
    }

    #[test]
    fn a_returning_screen_takes_its_windows_back() {
        let mut set = two_screens();
        set.reconcile(&[(n("A"), RA)], Some("A"), 10.0, GAP, RB, GO);
        set.reconcile(&[(n("A"), RA)], Some("A"), 12.0, GAP, RB, GO);
        set.reconcile(&ab(), Some("A"), 20.0, GAP, RB, GO);
        assert_eq!(set.screens.len(), 2);
        assert_eq!(set.screens[0].layout.all_clients(), vec![1]);
        let b = &set.screens[1].layout;
        assert_eq!(sorted(b.clients_on(0)), vec![10, 11, 13]);
        assert_eq!(b.clients_on(3), vec![12]);
        assert_eq!(b.clients_on(SCRATCHPAD), vec![14]);
        assert_eq!(b.float_rect(13), Some(LRect::new(2000.0, 100.0, 400.0, 300.0)));
        assert!(set.homes.is_empty());
    }

    #[test]
    fn a_moved_screen_moves_its_floats() {
        let mut set = two_screens();
        let moved = LRect::new(2020.0, 0.0, 1920.0, 1080.0);
        set.reconcile(&[(n("A"), RA), (n("B"), moved)], Some("A"), 1.0, GAP, RB, GO);
        assert_eq!(set.screens[1].rect, moved);
        assert_eq!(
            set.screens[1].layout.float_rect(13),
            Some(LRect::new(2100.0, 100.0, 400.0, 300.0))
        );
        assert_eq!(sorted(set.screens[1].layout.all_clients()), vec![10, 11, 12, 13, 14]);
    }

    #[test]
    fn going_back_to_the_fallback_merges_every_client() {
        let mut set = two_screens();
        set.active = 1;
        set.reconcile(&[(n(""), DESK)], None, 1.0, GAP, RB, GO);
        assert_eq!(set.screens.len(), 1);
        assert_eq!(set.screens[0].name, "");
        assert_eq!(set.screens[0].rect, DESK);
        assert_eq!(sorted(set.all_clients()), vec![1, 10, 11, 12, 13, 14]);
        let l = &set.screens[0].layout;
        assert_eq!(l.clients_on(3), vec![12]);
        assert_eq!(l.clients_on(SCRATCHPAD), vec![14]);
        // B lies inside the desk, so its float keeps its place.
        assert_eq!(l.float_rect(13), Some(LRect::new(2000.0, 100.0, 400.0, 300.0)));
        assert_eq!(set.active, 0);
        assert_eq!(set.main, 0);
    }

    #[test]
    fn active_is_clamped_after_a_removal() {
        let mut set = two_screens();
        set.active = 1;
        set.reconcile(&[(n("A"), RA)], Some("A"), 10.0, GAP, RB, GO);
        set.reconcile(&[(n("A"), RA)], Some("A"), 12.0, GAP, RB, GO);
        assert_eq!(set.screens.len(), 1);
        assert_eq!(set.active, 0);
        assert_eq!(set.active_layout().all_clients().len(), 6);
    }

    #[test]
    fn active_follows_its_screen_by_name() {
        let mut set = two_screens();
        set.active = 1;
        // A new screen on the left shifts B to index 2.
        let rl = LRect::new(-1920.0, 0.0, 1920.0, 1080.0);
        let new = vec![(n("L"), rl), (n("A"), RA), (n("B"), RB_)];
        set.reconcile(&new, Some("A"), 1.0, GAP, RB, GO);
        assert_eq!(set.screens[set.active].name, "B");
        assert_eq!(set.main, 1);
    }

    #[test]
    fn on_pointer_reports_only_a_crossing() {
        let mut set = two_screens();
        assert_eq!(set.active, 0);
        assert_eq!(set.on_pointer(100.0, 100.0), None);
        assert_eq!(set.on_pointer(2500.0, 100.0), Some(1));
        assert_eq!(set.active, 1);
        assert_eq!(set.on_pointer(2600.0, 200.0), None);
        assert_eq!(set.on_pointer(10.0, 10.0), Some(0));
        assert_eq!(set.active, 0);
    }

    #[test]
    fn focused_client_is_the_active_screens() {
        let mut set = two_screens();
        assert_eq!(set.focused_client(), Some(1));
        set.on_pointer(2500.0, 100.0);
        // B's last arrival on workspace 0 is the float 13.
        assert_eq!(set.focused_client(), Some(13));
    }

    #[test]
    fn move_to_screen_wraps_and_follows() {
        let mut set = two_screens();
        assert_eq!(set.move_to_screen(1, true, GAP, RB, GO), Some(1));
        assert_eq!(set.screen_of(1), Some(1));
        assert_eq!(set.active, 1);
        assert_eq!(set.focused_client(), Some(1));
        // Forward from the last screen wraps to the first.
        assert_eq!(set.move_to_screen(1, true, GAP, RB, GO), Some(0));
        assert_eq!(set.screen_of(1), Some(0));
        // Backward from the first wraps to the last.
        assert_eq!(set.move_to_screen(1, false, GAP, RB, GO), Some(1));
        assert_eq!(set.screen_of(1), Some(1));
    }

    #[test]
    fn move_to_screen_is_none_with_one_screen() {
        let mut set = fallback_set();
        assert_eq!(set.move_to_screen(1, true, GAP, RB, GO), None);
        assert_eq!(set.screen_of(1), Some(0));
    }

    #[test]
    fn move_to_screen_is_not_reverted_by_the_next_pointer_move_on_the_source_screen() {
        let mut set = two_screens();
        // The pointer is sitting on A (screen 0) when the key chord fires.
        assert_eq!(set.on_pointer(100.0, 100.0), None);
        assert_eq!(set.active, 0);
        // Move client 1 (on A) to B with the keyboard: active follows it.
        assert_eq!(set.move_to_screen(1, true, GAP, RB, GO), Some(1));
        assert_eq!(set.active, 1);
        // The mouse hasn't physically moved -- it's still over A. A
        // MouseMove at the same spot is not a crossing (the pointer never
        // left A), so it must not undo the keyboard move (task 4's I1 fix).
        assert_eq!(set.on_pointer(100.0, 100.0), None);
        assert_eq!(set.active, 1);
    }

    #[test]
    fn move_to_screen_of_an_unknown_client_is_none() {
        let mut set = two_screens();
        assert_eq!(set.move_to_screen(99, true, GAP, RB, GO), None);
    }
    #[test]
    fn rename_onto_a_right_hand_main_keeps_a_float_already_there() {
        let mut l = WmLayout::new();
        l.add_float(3, LRect::new(2500.0, 100.0, 400.0, 300.0), 0);
        let mut set = ScreenSet::new(l, DESK);
        set.reconcile(&ab(), Some("B"), 0.0, GAP, RB, GO);
        assert_eq!(
            set.screens[1].layout.float_rect(3),
            Some(LRect::new(2500.0, 100.0, 400.0, 300.0))
        );
    }

    #[test]
    fn merge_into_a_non_origin_survivor_keeps_every_position() {
        let mut set = two_screens();
        set.reconcile(&ab(), Some("B"), 1.0, GAP, RB, GO);
        assert_eq!(set.main, 1);
        set.screens[0].layout.add_float(5, LRect::new(300.0, 200.0, 400.0, 300.0), 0);
        set.reconcile(&[(n(""), DESK)], None, 2.0, GAP, RB, GO);
        let l = &set.screens[0].layout;
        // The survivor B (x=1920) lies inside the desk: its float stays.
        assert_eq!(l.float_rect(13), Some(LRect::new(2000.0, 100.0, 400.0, 300.0)));
        // A's float, merged in, stays too.
        assert_eq!(l.float_rect(5), Some(LRect::new(300.0, 200.0, 400.0, 300.0)));
        assert_eq!(sorted(set.all_clients()), vec![1, 5, 10, 11, 12, 13, 14]);
    }

    #[test]
    fn named_set_losing_a_screen_debounces_through_screen_rects_for() {
        let mut set = two_screens();
        assert!(set.is_named());
        let desk_a = RA;
        let names = vec![n("A")];
        let geoms = vec![RA];
        let new = screen_rects_for(per_screen_enabled(true, false), set.is_named(), &names, &geoms, desk_a);
        assert_eq!(new, vec![(n("A"), RA)]);
        set.reconcile(&new, Some("A"), 10.0, GAP, RB, GO);
        assert_eq!(set.screens.len(), 2);
        assert_eq!(set.live_count(), 1);
        assert_eq!(set.screens[1].name, "B");
        assert_eq!(sorted(set.screens[1].layout.all_clients()), vec![10, 11, 12, 13, 14]);
        set.reconcile(&new, Some("A"), 11.5, GAP, RB, GO);
        assert_eq!(set.live_count(), 1);
        assert_eq!(set.screens.len(), 2);
        set.reconcile(&new, Some("A"), 12.0, GAP, RB, GO);
        assert_eq!(set.screens.len(), 1);
        assert_eq!(set.live_count(), 1);
        assert_eq!(sorted(set.all_clients()), vec![1, 10, 11, 12, 13, 14]);
        assert!(set.is_named());
    }

    #[test]
    fn an_empty_input_changes_nothing() {
        let mut set = two_screens();
        set.reconcile(&[], Some("A"), 10.0, GAP, RB, GO);
        set.reconcile(&[], Some("A"), 20.0, GAP, RB, GO);
        assert_eq!(set.live_count(), 2);
        assert_eq!(sorted(set.screens[1].layout.all_clients()), vec![10, 11, 12, 13, 14]);
    }

    #[test]
    fn pending_screens_are_never_active_or_movable() {
        let mut set = two_screens();
        set.reconcile(&[(n("A"), RA)], Some("A"), 10.0, GAP, RB, GO);
        assert_eq!(set.live_count(), 1);
        // 10 sits on the pending B.
        assert_eq!(set.screen_of(10), Some(1));
        assert_eq!(set.activate_screen_of(10), None);
        assert_eq!(set.active, 0);
        assert_eq!(set.move_to_screen(10, true, GAP, RB, GO), None);
        assert_eq!(set.on_pointer(2500.0, 100.0), None);
        assert_eq!(set.active, 0);
        assert_eq!(set.activate_screen_of(1), Some(0));
    }

    #[test]
    fn the_dock_is_reserved_on_the_main_screen_only() {
        let mut set = ScreenSet::new(WmLayout::new(), DESK);
        set.reconcile(&ab(), Some("B"), 0.0, GAP, 60.0, 10.0);
        assert_eq!(set.main, 1);
        assert_eq!(set.reserved_for(0, 60.0), 0.0);
        assert_eq!(set.reserved_for(1, 60.0), 60.0);
        assert_eq!(set.area(0, 60.0, 10.0), screen_area(RA, 0.0, 10.0));
        assert_eq!(set.area(1, 60.0, 10.0), screen_area(RB_, 60.0, 10.0));
        // A float as tall as the screen moved onto the non-main screen is
        // fitted into the full height there, not one shortened by the dock.
        let tall = LRect::new(2000.0, 10.0, 400.0, 1060.0);
        set.screens[1].layout.add_float(5, tall, 0);
        set.active = 1;
        assert_eq!(set.move_to_screen(5, true, GAP, 60.0, 10.0), Some(0));
        assert_eq!(set.screens[0].layout.float_rect(5).map(|r| r.h), Some(1060.0));
    }

    #[test]
    fn the_merged_desk_is_main_and_keeps_the_dock() {
        let mut set = two_screens();
        set.reconcile(&ab(), Some("B"), 1.0, GAP, RB, GO);
        assert_eq!(set.main, 1);
        set.reconcile(&[(n(""), DESK)], None, 2.0, GAP, RB, GO);
        assert_eq!(set.main, 0);
        assert_eq!(set.reserved_for(0, 60.0), 60.0);
    }

    #[test]
    fn a_single_fallback_entry_tiles_like_the_bare_layout() {
        // Single-screen invariance: one "" entry over the desk hands out
        // exactly the tile rects the bare WmLayout does for the same area.
        let area = LRect::new(10.0, 36.0, 1900.0, 1000.0);
        let gap = 8.0;
        let mut bare = WmLayout::new();
        let mut inner = WmLayout::new();
        for c in 1..=4 {
            bare.insert(c, area, gap);
            inner.insert(c, area, gap);
        }
        bare.add_float(9, LRect::new(300.0, 200.0, 400.0, 300.0), 0);
        inner.add_float(9, LRect::new(300.0, 200.0, 400.0, 300.0), 0);
        let set = ScreenSet::new(inner, DESK);
        assert_eq!(set.live_count(), 1);
        assert_eq!(set.active_layout().rects(area, gap), bare.rects(area, gap));
        assert_eq!(set.active_layout().groups(area, gap).len(), bare.groups(area, gap).len());
        // The fallback's own area is the WM's single-desk area.
        assert_eq!(set.area(0, 60.0, 10.0), screen_area(DESK, 60.0, 10.0));
    }

    #[test]
    fn a_float_dropped_over_another_screen_moves_there_in_place() {
        let mut set = two_screens();
        // Float 13 (on B) was dragged so its centre sits over A.
        set.screens[1].layout.set_float_rect(13, LRect::new(300.0, 100.0, 400.0, 300.0));
        assert!(set.drop_on_screen(13, 0, GAP, RB, GO));
        assert_eq!(set.screen_of(13), Some(0));
        assert_eq!(set.active, 0);
        assert_eq!(
            set.screens[0].layout.float_rect(13),
            Some(LRect::new(300.0, 100.0, 400.0, 300.0))
        );
        // Already there, unknown, or a screen that is not live: nothing.
        assert!(!set.drop_on_screen(13, 0, GAP, RB, GO));
        assert!(!set.drop_on_screen(99, 1, GAP, RB, GO));
        assert!(!set.drop_on_screen(13, 5, GAP, RB, GO));
    }

    #[test]
    fn a_focus_change_is_not_undone_by_the_pointer_staying_put() {
        let mut set = two_screens();
        assert_eq!(set.on_pointer(100.0, 100.0), None);
        // A dock click / bar entry focuses a window on B; the pointer is
        // still on A.
        assert_eq!(set.activate_screen_of(12), Some(1));
        assert_eq!(set.on_pointer(110.0, 105.0), None);
        assert_eq!(set.active, 1);
        // Only a real crossing moves it back.
        assert_eq!(set.on_pointer(2500.0, 100.0), None);
        assert_eq!(set.on_pointer(100.0, 100.0), Some(0));
        assert_eq!(set.active, 0);
    }

    #[test]
    fn reconcile_forgets_the_pointer_screen() {
        let mut set = two_screens();
        assert_eq!(set.on_pointer(2500.0, 100.0), Some(1));
        set.reconcile(&ab(), Some("A"), 1.0, GAP, RB, GO);
        set.active = 0;
        // Indices may have shifted: the next sighting counts as a crossing.
        assert_eq!(set.on_pointer(2500.0, 100.0), Some(1));
    }

    #[test]
    fn a_desk_clip_that_narrows_and_restores_moves_no_window() {
        let mut set = two_screens();
        set.screens[0].layout.add_float(7, LRect::new(10.0, 100.0, 1800.0, 300.0), 0);
        let f13 = set.screens[1].layout.float_rect(13);
        // The AI pane opens: A's clipped rect loses 600 on the left.
        let narrow = vec![(n("A"), LRect::new(600.0, 0.0, 1320.0, 1080.0)), (n("B"), RB_)];
        assert!(set.set_clip(&narrow));
        assert_eq!(set.rects(), vec![LRect::new(600.0, 0.0, 1320.0, 1080.0), RB_]);
        assert!(set.set_clip(&ab()));
        assert_eq!(set.rects(), vec![RA, RB_]);
        assert_eq!(set.screens[0].layout.float_rect(7), Some(LRect::new(10.0, 100.0, 1800.0, 300.0)));
        assert_eq!(set.screens[1].layout.float_rect(13), f13);
        // Not the live screens in order: refused, nothing changed.
        assert!(!set.set_clip(&[(n("B"), RB_), (n("A"), RA)]));
        assert!(!set.set_clip(&[(n("A"), RA)]));
        assert_eq!(set.rects(), vec![RA, RB_]);
    }

    #[test]
    fn activate_screen_of_switches_to_a_live_screen() {
        let mut set = two_screens();
        assert_eq!(set.activate_screen_of(12), Some(1));
        assert_eq!(set.active, 1);
        assert_eq!(set.activate_screen_of(99), None);
        assert_eq!(set.active, 1);
    }

    // --- Task 7: surfaces on the active screen, the dock on the main ----

    #[test]
    fn one_screen_gives_no_surface_or_dock_target() {
        let set = fallback_set();
        assert_eq!(set.surface_rect(0), None);
        assert_eq!(set.main_rect(), None);
    }

    #[test]
    fn surfaces_target_their_screen_and_the_dock_the_main_one() {
        let mut set = two_screens();
        assert_eq!(set.surface_rect(0), Some(RA));
        assert_eq!(set.surface_rect(1), Some(RB_));
        assert_eq!(set.main_rect(), Some(RA));
        // A stale index (a screen that went away) takes the active one.
        set.active = 1;
        assert_eq!(set.surface_rect(7), Some(RB_));
        set.reconcile(&ab(), Some("B"), 0.0, GAP, RB, GO);
        assert_eq!(set.main_rect(), Some(RB_));
    }

    #[test]
    fn a_pending_screen_is_no_surface_target() {
        let mut set = two_screens();
        set.active = 0;
        set.reconcile(&[(n("A"), RA)], Some("A"), 10.0, GAP, RB, GO);
        // B waits out its debounce: one live screen, today's whole desk.
        assert_eq!(set.live_count(), 1);
        assert_eq!(set.surface_rect(1), None);
        assert_eq!(set.main_rect(), None);
    }

    fn to_rect(r: LRect) -> makepad_widgets::Rect {
        makepad_widgets::rect(r.x, r.y, r.w, r.h)
    }

    fn inside(outer: makepad_widgets::Rect, inner: makepad_widgets::Rect) -> bool {
        inner.pos.x >= outer.pos.x - 1e-6
            && inner.pos.y >= outer.pos.y - 1e-6
            && inner.pos.x + inner.size.x <= outer.pos.x + outer.size.x + 1e-6
            && inner.pos.y + inner.size.y <= outer.pos.y + outer.size.y + 1e-6
    }

    /// The menu card laid out in a screen that does not start at 0 (the
    /// right screen of two, under the bar strip) stays inside it, for
    /// every desktop style, centred or anchored to a bar module, a module
    /// at either end of the segment included.
    #[test]
    fn the_menu_card_stays_inside_a_target_that_does_not_start_at_zero() {
        use crate::desktop::DesktopStyle;
        use crate::shell::menu::{menu_card_layout, MenuCardSpec};
        let screen = makepad_widgets::rect(1920.0, 26.0, 1280.0, 974.0);
        let dividers = vec![false, true, false, false, false, false, false, false];
        let anchors = [
            None,
            Some(makepad_widgets::rect(1924.0, 0.0, 40.0, 26.0)),
            Some(makepad_widgets::rect(3180.0, 0.0, 40.0, 26.0)),
            // A module of the left segment: still clamped onto this screen.
            Some(makepad_widgets::rect(1700.0, 0.0, 40.0, 26.0)),
        ];
        for style in [
            DesktopStyle::Omarchy,
            DesktopStyle::Macos,
            DesktopStyle::Windows,
            DesktopStyle::Windows2000,
            DesktopStyle::NextStep,
        ] {
            for anchor in anchors {
                for filter_empty in [true, false] {
                    let spec = MenuCardSpec {
                        anchor,
                        style,
                        filter_empty,
                        row_height: 30.0,
                        dividers: &dividers,
                        empty_block_h: 60.0,
                        panel_padding: 14.0,
                        gaps_out: 10.0,
                        frozen_top: None,
                    };
                    let (card, visible) = menu_card_layout(&spec, screen);
                    assert!(visible >= 1);
                    assert!(
                        inside(screen, card),
                        "{style:?} anchor {anchor:?}: {card:?} outside {screen:?}"
                    );
                }
            }
        }
    }

    /// A flyout anchored to a module near the seam is clamped inside its
    /// own segment's screen, not centred across the seam.
    #[test]
    fn a_panel_anchor_is_clamped_inside_its_screen() {
        use crate::shell::panels::panel_card_rect;
        let left = to_rect(LRect::new(0.0, 26.0, 1920.0, 1054.0));
        let right = to_rect(LRect::new(1920.0, 26.0, 1920.0, 1054.0));
        let w = 360.0;
        let margin = 10.0;
        // The left screen's last module, right at the seam.
        let a = makepad_widgets::rect(1890.0, 0.0, 28.0, 26.0);
        let card = panel_card_rect(a, left, w, 400.0, margin, 26.0);
        assert!(inside(left, card), "{card:?}");
        assert_eq!(card.pos.x + card.size.x, 1920.0 - margin);
        // The right screen's first module, right at the seam.
        let b = makepad_widgets::rect(1922.0, 0.0, 28.0, 26.0);
        let card = panel_card_rect(b, right, w, 400.0, margin, 26.0);
        assert!(inside(right, card), "{card:?}");
        assert_eq!(card.pos.x, 1920.0 + margin);
        // In the middle of a screen: centred on the module, under the bar.
        let c = makepad_widgets::rect(2800.0, 0.0, 40.0, 26.0);
        let card = panel_card_rect(c, right, w, 400.0, margin, 26.0);
        assert_eq!(card.pos.x, (2820.0f64 - 180.0).floor());
        assert_eq!(card.pos.y, 36.0);
        // A height past the screen's bottom is cut at the margin.
        let card = panel_card_rect(c, right, w, 5000.0, margin, 26.0);
        assert!(inside(right, card), "{card:?}");
    }

    /// The one dock lists every live screen's shown windows, screen by
    /// screen, and does not change as the pointer makes another screen
    /// active; a screen waiting out its removal is not listed.
    #[test]
    fn the_dock_lists_every_live_screens_windows_whichever_is_active() {
        let mut set = two_screens();
        let one_screen = |s: &ScreenSet, i: usize| {
            let l = &s.screens[i].layout;
            l.clients_on(l.active)
        };
        let mut want = one_screen(&set, 0);
        want.extend(one_screen(&set, 1));
        assert_eq!(sorted(want.clone()), vec![1, 10, 11, 13]);
        set.active = 0;
        assert_eq!(set.shown_clients(), want);
        set.active = 1;
        assert_eq!(set.shown_clients(), want);
        // Screen order first: A's window leads.
        assert_eq!(set.shown_clients()[0], 1);
        // B pending: only A's.
        set.reconcile(&[(n("A"), RA)], Some("A"), 10.0, GAP, RB, GO);
        assert_eq!(set.shown_clients(), vec![1]);
        // One fallback screen: exactly its own list, as before.
        let set = fallback_set();
        assert_eq!(set.shown_clients(), one_screen(&set, 0));
    }
}
