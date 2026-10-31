//! Spaces: the one place a display's windows are grouped.
//!
//! A display has an ORDERED list of spaces, and shows one at a time:
//!
//! - a **desktop space** is a full spacewarp desktop with its own windows,
//!   parked zones, background state and layout history. The person makes and
//!   removes them (always at least one);
//! - a **fullscreen space** holds exactly one window, shown 1:1 with no
//!   chrome and no background. It exists while that window is fullscreen and
//!   sits right after the desktop it came from.
//!
//! Each space is backed by one *slot* of the display's [`WmLayout`]
//! (`crate::layout`): the slot holds the space's tile tree and floats.
//! Desktop spaces use slots `0..MAX_DESKTOPS`, fullscreen spaces
//! `MAX_DESKTOPS..SLOTS`, so a slot number never means two things. The
//! list here is pure data (unit-tested below); `layout.rs` moves windows
//! between slots, `slide.rs` / `strip.rs` / `sheet.rs` are what is shown, and
//! `persist.rs` is what survives a restart.
/// Desktop spaces a display can have.
pub const MAX_DESKTOPS: usize = 12;
/// Fullscreen spaces a display can have at once.
pub const MAX_FULLSCREEN: usize = 12;
/// Layout slots in all (the scratchpad comes after them).
pub const SLOTS: usize = MAX_DESKTOPS + MAX_FULLSCREEN;

pub mod app;
pub mod sheet;
pub mod slide;
pub mod strip;

pub type ClientId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Desktop,
    /// One window, shown fullscreen; `home` is the desktop slot it returns to.
    Fullscreen { client: ClientId, home: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    pub slot: usize,
    pub kind: Kind,
}

impl Space {
    pub fn is_desktop(&self) -> bool {
        self.kind == Kind::Desktop
    }

    pub fn fullscreen_client(&self) -> Option<ClientId> {
        match self.kind {
            Kind::Fullscreen { client, .. } => Some(client),
            Kind::Desktop => None,
        }
    }
}

/// The ordered spaces of one display and which one is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceList {
    items: Vec<Space>,
    cur: usize,
}

impl Default for SpaceList {
    fn default() -> Self {
        Self::new()
    }
}

impl SpaceList {
    /// One empty desktop.
    pub fn new() -> Self {
        Self { items: vec![Space { slot: 0, kind: Kind::Desktop }], cur: 0 }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn items(&self) -> &[Space] {
        &self.items
    }

    pub fn get(&self, i: usize) -> Option<Space> {
        self.items.get(i).copied()
    }

    /// Index of the space shown.
    pub fn cur(&self) -> usize {
        self.cur
    }

    pub fn current(&self) -> Space {
        self.items[self.cur]
    }

    /// The slot of the space shown.
    pub fn active_slot(&self) -> usize {
        self.items[self.cur].slot
    }

    pub fn index_of_slot(&self, slot: usize) -> Option<usize> {
        self.items.iter().position(|s| s.slot == slot)
    }

    pub fn index_of_fullscreen(&self, client: ClientId) -> Option<usize> {
        self.items.iter().position(|s| s.fullscreen_client() == Some(client))
    }

    pub fn desktop_count(&self) -> usize {
        self.items.iter().filter(|s| s.is_desktop()).count()
    }

    /// The 1-based number of the desktop at index `i` among the desktops.
    pub fn desktop_number(&self, i: usize) -> Option<usize> {
        self.items.get(i).filter(|s| s.is_desktop())?;
        Some(self.items[..=i].iter().filter(|s| s.is_desktop()).count())
    }

    /// Index of the `n`-th desktop (0-based among the desktops).
    pub fn desktop_index(&self, n: usize) -> Option<usize> {
        self.items.iter().enumerate().filter(|(_, s)| s.is_desktop()).nth(n).map(|(i, _)| i)
    }

    /// Show the space at `i`. False when there is none.
    pub fn goto(&mut self, i: usize) -> bool {
        if i >= self.items.len() {
            return false;
        }
        self.cur = i;
        true
    }

    /// The index `dir` (-1 / +1) steps from the one shown; none at the ends
    /// (the strip of spaces does not wrap).
    pub fn neighbour(&self, dir: i32) -> Option<usize> {
        let to = self.cur as i64 + dir as i64;
        (to >= 0 && (to as usize) < self.items.len()).then_some(to as usize)
    }

    fn free_slot(&self, range: std::ops::Range<usize>) -> Option<usize> {
        range.into_iter().find(|s| self.index_of_slot(*s).is_none())
    }

    /// A new empty desktop at the end of the list. None at the limit.
    pub fn push_desktop(&mut self) -> Option<usize> {
        let slot = self.free_slot(0..MAX_DESKTOPS)?;
        self.items.push(Space { slot, kind: Kind::Desktop });
        Some(self.items.len() - 1)
    }

    /// Make sure a desktop with exactly this slot exists (windows arriving
    /// from another display keep their desktop number); it goes at the end.
    pub fn ensure_desktop_slot(&mut self, slot: usize) -> bool {
        if slot >= MAX_DESKTOPS {
            return false;
        }
        if self.index_of_slot(slot).is_none() {
            self.items.push(Space { slot, kind: Kind::Desktop });
        }
        true
    }

    /// A fullscreen space for `client`, right after the desktop of slot
    /// `home`. Returns its index and slot. None at the limit or when the
    /// client already has one.
    pub fn add_fullscreen(&mut self, client: ClientId, home: usize) -> Option<(usize, usize)> {
        if self.index_of_fullscreen(client).is_some() {
            return None;
        }
        let slot = self.free_slot(MAX_DESKTOPS..SLOTS)?;
        let after = self.index_of_slot(home).unwrap_or(self.cur);
        // After the desktop, and after fullscreen spaces already there? No:
        // right after the desktop, so the newest is nearest to it.
        let at = after + 1;
        self.items.insert(at, Space { slot, kind: Kind::Fullscreen { client, home } });
        if self.cur >= at {
            self.cur += 1;
        }
        Some((at, slot))
    }

    /// Remove the space at `i`; the one shown stays shown, or when it was
    /// the one removed the previous one is. Never removes the last desktop.
    pub fn remove(&mut self, i: usize) -> Option<Space> {
        let sp = self.get(i)?;
        if sp.is_desktop() && self.desktop_count() <= 1 {
            return None;
        }
        self.items.remove(i);
        if i < self.cur || (i == self.cur && self.cur > 0) {
            self.cur -= 1;
        }
        self.cur = self.cur.min(self.items.len() - 1);
        Some(sp)
    }

    /// The desktop slot windows of the removed desktop at `i` move to: the
    /// nearest desktop before it, else the first one after.
    pub fn previous_desktop_slot(&self, i: usize) -> Option<usize> {
        self.items[..i.min(self.items.len())]
            .iter()
            .rev()
            .find(|s| s.is_desktop())
            .or_else(|| self.items.iter().skip(i + 1).find(|s| s.is_desktop()))
            .map(|s| s.slot)
    }

    /// Fullscreen spaces whose home is `from` now return to `to`.
    pub fn rehome(&mut self, from: usize, to: usize) {
        for s in &mut self.items {
            if let Kind::Fullscreen { home, .. } = &mut s.kind {
                if *home == from {
                    *home = to;
                }
            }
        }
    }

    /// Move the space at `from` so it sits at index `to`; the one shown
    /// stays shown.
    pub fn move_space(&mut self, from: usize, to: usize) -> bool {
        if from >= self.items.len() || to >= self.items.len() || from == to {
            return false;
        }
        let shown = self.items[self.cur].slot;
        let sp = self.items.remove(from);
        self.items.insert(to, sp);
        self.cur = self.index_of_slot(shown).unwrap_or(0);
        true
    }

    /// The desktop slot a fullscreen space returns to.
    pub fn home_of(&self, i: usize) -> Option<usize> {
        match self.items.get(i)?.kind {
            Kind::Fullscreen { home, .. } => Some(home),
            Kind::Desktop => None,
        }
    }

    /// Rebuild a list of `n` desktops (their slots 0..n), the `cur`-th shown
    /// (used when the persisted spaces are read back).
    pub fn with_desktops(n: usize, cur: usize) -> Self {
        let n = n.clamp(1, MAX_DESKTOPS);
        Self {
            items: (0..n).map(|slot| Space { slot, kind: Kind::Desktop }).collect(),
            cur: cur.min(n - 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(l: &SpaceList) -> Vec<(usize, bool)> {
        l.items().iter().map(|s| (s.slot, s.is_desktop())).collect()
    }

    #[test]
    fn a_display_starts_with_one_desktop_and_never_loses_the_last() {
        let mut l = SpaceList::new();
        assert_eq!((l.len(), l.cur(), l.desktop_count()), (1, 0, 1));
        assert!(l.remove(0).is_none());
        let i = l.push_desktop().unwrap();
        assert_eq!((i, l.items()[i].slot), (1, 1));
        assert!(l.remove(0).is_some());
        assert_eq!(l.len(), 1);
        assert!(l.remove(0).is_none());
    }

    #[test]
    fn desktops_are_limited_and_slots_are_not_reused_while_alive() {
        let mut l = SpaceList::new();
        for _ in 1..MAX_DESKTOPS {
            assert!(l.push_desktop().is_some());
        }
        assert!(l.push_desktop().is_none());
        l.remove(3);
        // The freed slot comes back.
        let i = l.push_desktop().unwrap();
        assert_eq!(l.items()[i].slot, 3);
    }

    #[test]
    fn a_fullscreen_space_sits_right_after_its_desktop() {
        let mut l = SpaceList::new();
        l.push_desktop();
        l.push_desktop();
        // From desktop 1 (slot 1, index 1).
        l.goto(1);
        let (at, slot) = l.add_fullscreen(77, 1).unwrap();
        assert_eq!((at, slot), (2, MAX_DESKTOPS));
        assert_eq!(kinds(&l), vec![(0, true), (1, true), (MAX_DESKTOPS, false), (2, true)]);
        // The one shown was before it: still index 1.
        assert_eq!(l.cur(), 1);
        // Showing it, then a second fullscreen from the same desktop goes
        // right after the desktop too.
        l.goto(2);
        assert!(l.add_fullscreen(77, 1).is_none(), "one space per window");
        let (at2, _) = l.add_fullscreen(78, 1).unwrap();
        assert_eq!(at2, 2);
        assert_eq!(l.cur(), 3, "the shown space keeps being shown");
        assert_eq!(l.current().fullscreen_client(), Some(77));
        assert_eq!(l.index_of_fullscreen(78), Some(2));
    }

    #[test]
    fn removing_keeps_the_shown_space_or_steps_back() {
        let mut l = SpaceList::new();
        for _ in 0..3 {
            l.push_desktop();
        }
        l.goto(2);
        l.remove(0);
        assert_eq!((l.cur(), l.active_slot()), (1, 2));
        l.remove(1);
        assert_eq!((l.cur(), l.active_slot()), (0, 1));
        l.remove(0);
        assert_eq!((l.cur(), l.len()), (0, 1));
    }

    #[test]
    fn windows_of_a_removed_desktop_go_to_the_previous_one() {
        let mut l = SpaceList::new();
        l.push_desktop();
        l.push_desktop();
        l.add_fullscreen(5, 1);
        // List: d0 d1 f d2.
        assert_eq!(l.previous_desktop_slot(3), Some(1));
        assert_eq!(l.previous_desktop_slot(1), Some(0));
        assert_eq!(l.previous_desktop_slot(0), Some(1));
        l.rehome(1, 0);
        assert_eq!(l.home_of(2), Some(0));
    }

    #[test]
    fn reordering_keeps_the_shown_space() {
        let mut l = SpaceList::new();
        for _ in 0..3 {
            l.push_desktop();
        }
        l.goto(1);
        assert!(l.move_space(1, 3));
        assert_eq!((l.cur(), l.active_slot()), (3, 1));
        assert!(l.move_space(0, 2));
        assert_eq!(l.active_slot(), 1);
        assert_eq!(l.items().iter().map(|s| s.slot).collect::<Vec<_>>(), vec![2, 3, 0, 1]);
        assert!(!l.move_space(2, 2) && !l.move_space(9, 0));
    }

    #[test]
    fn neighbours_do_not_wrap_and_desktops_are_numbered_among_themselves() {
        let mut l = SpaceList::new();
        l.push_desktop();
        l.add_fullscreen(9, 0);
        // d0 f d1
        assert_eq!(l.neighbour(-1), None);
        assert_eq!(l.neighbour(1), Some(1));
        l.goto(2);
        assert_eq!(l.neighbour(1), None);
        assert_eq!((l.desktop_number(0), l.desktop_number(1), l.desktop_number(2)), (Some(1), None, Some(2)));
        assert_eq!((l.desktop_index(0), l.desktop_index(1), l.desktop_index(2)), (Some(0), Some(2), None));
    }

    #[test]
    fn a_desktop_from_another_display_keeps_its_number() {
        let mut l = SpaceList::new();
        assert!(l.ensure_desktop_slot(3));
        assert!(l.ensure_desktop_slot(3));
        assert_eq!(l.len(), 2);
        assert!(!l.ensure_desktop_slot(MAX_DESKTOPS));
        let r = SpaceList::with_desktops(3, 9);
        assert_eq!((r.len(), r.cur()), (3, 2));
    }
}

/// What is on show of the spaces besides the shown space itself: the running
/// slides, the strip, the cheat sheet, and the windows a growing fullscreen
/// window still has under it.
#[derive(Default)]
pub struct Ui {
    pub slides: slide::Slides,
    pub strip: strip::Strip,
    /// The cheat sheet is open.
    pub sheet: bool,
    /// (display, client, rect): windows of the space just left, drawn under a
    /// fullscreen window while it grows, then dropped.
    pub underlay: Vec<(usize, ClientId, crate::layout::LRect)>,
    /// When the underlay was set (`host::now`).
    pub underlay_at: f64,
    /// The cached captures are stale (windows moved between spaces).
    pub stale: bool,
}

impl Ui {
    /// Advance the motion; true while anything still moves.
    pub fn step(&mut self, dt: f64) -> bool {
        let slide = self.slides.step(dt);
        let strip = self.strip.step(dt);
        slide || strip
    }

    /// Something is moving, or a capture is being used.
    pub fn busy(&self) -> bool {
        self.slides.any() || !self.strip.ease.settled()
    }
}
