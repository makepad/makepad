//! Lane D. The overlay layer: menus (menu bar, editor-type dropdowns,
//! right-click context menus) and the Z shading pie.
//!
//! Both live at the very end of the shell's overlay stack — after every panel,
//! before the `TipLayer` — for the same reason `TipLayer` does: an overlay
//! draw list floats over every area and splitter, and being last in the tree
//! means the layer wins hover against whatever sits underneath it.
//!
//! Anyone can raise one without owning it. A control emits
//! `FabUiAction::OpenMenu { .. }` with the item list and the anchor rect it
//! measured from its own drawn area (never mid-pass turtle state — the
//! overlay-layer law), and reads `FabUiAction::MenuPicked { .. }` back out of
//! the next actions pass. No shared state, no `api.rs` surface.

use makepad_widgets::*;

/// Which pre-declared glyph a menu row shows in its icon column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MenuIcon {
    #[default]
    None,
    Check,
    Dot,
    Viewport,
    Outliner,
    Properties,
    Sheets,
    Info,
    Render,
    Tours,
}

/// One row of a menu. `id` comes back in [`FabUiAction::MenuPicked`].
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub id: LiveId,
    pub label: String,
    pub shortcut: String,
    pub icon: MenuIcon,
    pub enabled: bool,
    /// A 1 px rule instead of a row; `id`/`label` are ignored.
    pub separator: bool,
    /// Flyout rows. Non-empty = this row opens a submenu instead of firing.
    pub submenu: Vec<MenuItem>,
}

impl MenuItem {
    pub fn new(id: LiveId, label: &str) -> Self {
        MenuItem {
            id,
            label: label.to_string(),
            shortcut: String::new(),
            icon: MenuIcon::None,
            enabled: true,
            separator: false,
            submenu: Vec::new(),
        }
    }

    pub fn key(mut self, shortcut: &str) -> Self {
        self.shortcut = shortcut.to_string();
        self
    }

    pub fn icon(mut self, icon: MenuIcon) -> Self {
        self.icon = icon;
        self
    }

    pub fn checked(mut self, on: bool) -> Self {
        if on {
            self.icon = MenuIcon::Check;
        }
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// `on` = enabled. Reads better than `if !on { it = it.disabled() }` at
    /// every call site that has a live predicate to hand.
    pub fn enabled_if(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// A radio row: the dot marks the active one of a group.
    pub fn radio(mut self, on: bool) -> Self {
        if on {
            self.icon = MenuIcon::Dot;
        }
        self
    }

    /// Turn this row into a flyout parent. Firing it opens `rows` instead of
    /// emitting a pick.
    pub fn flyout(mut self, rows: Vec<MenuItem>) -> Self {
        self.submenu = rows;
        self
    }

    pub fn sep() -> Self {
        MenuItem {
            id: LiveId(0),
            label: String::new(),
            shortcut: String::new(),
            icon: MenuIcon::None,
            enabled: false,
            separator: true,
            submenu: Vec::new(),
        }
    }
}

/// Where the menu hangs off its anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPlace {
    /// Menu-bar style: below the anchor, left edges aligned.
    Below,
    /// Context style: at the pointer, growing right and down.
    At,
    /// Popover style: below the anchor, right edges aligned.
    BelowRight,
}

/// Lane D's internal chrome bus. Never crosses into `api.rs`.
#[derive(Debug)]
pub enum FabUiAction {
    /// Raise a menu. `owner` is any id the requester picks so it can tell its
    /// own menus apart when the pick comes back.
    OpenMenu {
        owner: LiveId,
        items: Vec<MenuItem>,
        anchor: Rect,
        place: MenuPlace,
    },
    /// A row was chosen. The same pass carries the `MenuClosed` for it.
    MenuPicked { owner: LiveId, id: LiveId },
    /// A root menu is up for `owner`. The layer raises this on every open;
    /// when the menu replaced another one, that one's `MenuClosed` precedes
    /// it in the same pass.
    MenuOpened { owner: LiveId },
    /// `owner`'s menu is down, whatever took it down: a pick, Escape, a press
    /// outside, another menu opening over it, the window losing focus.
    /// Popup buttons mirror `MenuOpened` / `MenuClosed` (see [`open_after`])
    /// instead of keeping an open flag of their own — a flag has to be
    /// cleared on every one of those paths, and the path that was forgotten
    /// is the highlight that lingers.
    MenuClosed { owner: LiveId },
    /// The press that dismissed a menu landed at `at`, outside every bubble.
    /// The menu's grab hid that press from whatever sits there, so the
    /// control under `at` treats it as its own: a dropdown next to the open
    /// one opens on that click instead of needing a second — unless it is the
    /// control whose menu just closed, for which the press was the toggle.
    MenuClickAway { at: Vec2d },
    /// Raise the radial shading menu centred on `at`.
    OpenPie { owner: LiveId, at: Vec2d, items: Vec<PieItem> },
    /// A pie wedge was chosen.
    PiePicked { owner: LiveId, id: LiveId },
    /// A `FabDropdownButton` was pressed. Whoever owns that `tag` builds the
    /// item list and raises the menu — the button itself has no opinion about
    /// what is in it, which is what lets lane G's header dropdown work without
    /// lane G writing a line of menu code.
    DropdownClicked { tag: LiveId, anchor: Rect },
    /// Raise the colour-picker popover (`FabColorPickerLayer`, shell overlay
    /// stack) anchored to a swatch's drawn rect. The layer mirrors the menu
    /// contract: it broadcasts `MenuOpened` / `MenuClosed` for `owner`, so
    /// every popup button agrees on what is open.
    OpenColorPicker {
        owner: LiveId,
        anchor: Rect,
        rgba: [f32; 4],
        with_alpha: bool,
    },
    /// Live colour while the picker's wheel / rows / hex are being edited.
    ColorPickerChanged { owner: LiveId, rgba: [f32; 4] },
    /// A commit point (release, Enter, click-away close, Escape-revert).
    ColorPickerEnded { owner: LiveId, rgba: [f32; 4] },
}

/// Read every `FabUiAction` out of an actions pass.
pub fn ui_actions(actions: &Actions) -> impl Iterator<Item = &FabUiAction> {
    actions.iter().filter_map(|a| a.downcast_ref::<FabUiAction>())
}

// ===========================================================================
// Which popup is open — the one fact every popup button shares
// ===========================================================================

/// The shell's single open popup. `FabMenuLayer` owns the instance and routes
/// every change of "which menu is up" through [`OpenPopup::set`],
/// broadcasting the result as `MenuClosed` / `MenuOpened`; every popup button
/// mirrors that broadcast with [`open_after`]. "Exactly one control looks
/// open, and only while its menu is up" is then a property of the bus, not of
/// each button's luck with the events a modal grab lets through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenPopup {
    owner: Option<LiveId>,
}

/// One step of the broadcast, in the order it must be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupChange {
    Closed(LiveId),
    Opened(LiveId),
}

impl OpenPopup {
    pub fn owner(&self) -> Option<LiveId> {
        self.owner
    }

    /// Move to `next` and return what to broadcast: the close of the
    /// previous menu before the open of the new one, nothing when nothing
    /// changed.
    pub fn set(&mut self, next: Option<LiveId>) -> Vec<PopupChange> {
        let mut changes = Vec::new();
        if self.owner == next {
            return changes;
        }
        if let Some(old) = self.owner.take() {
            changes.push(PopupChange::Closed(old));
        }
        if let Some(new) = next {
            changes.push(PopupChange::Opened(new));
        }
        self.owner = next;
        changes
    }
}

/// A button's mirror of the broadcast: given one pass's `FabUiAction`s in
/// order, is `owner`'s menu up after them? `None` when the pass said nothing
/// about menus. Any `MenuClosed` answers "no" — only one menu is ever up, so
/// a close of somebody else's menu cannot leave this one open either.
pub fn open_after<'a>(
    events: impl IntoIterator<Item = &'a FabUiAction>,
    owner: LiveId,
) -> Option<bool> {
    let mut open = None;
    for a in events {
        match a {
            FabUiAction::MenuOpened { owner: o } => open = Some(*o == owner),
            FabUiAction::MenuClosed { .. } => open = Some(false),
            _ => {}
        }
    }
    open
}

/// A wedge of the pie menu.
#[derive(Clone, Debug)]
pub struct PieItem {
    pub id: LiveId,
    pub label: String,
    pub active: bool,
}

// ===========================================================================
// The layer itself lives in `menu.rs`
// ===========================================================================
//
// `popover.rs` keeps the vocabulary — `MenuItem`, `MenuIcon`, `MenuPlace`,
// `FabUiAction` — because every header dropdown in the app builds against it.
// The overlay widget that draws menus, tracks hover across levels, walks the
// keyboard and opens flyouts is `ui::menu::FabMenuLayer`; it consumes exactly
// the `OpenMenu` requests raised through `open_menu` below.

/// Registration entry point kept where `ui/mod.rs` expects it; the DSL it
/// registers now belongs to `menu.rs`.
pub fn script_mod(vm: &mut ScriptVm) {
    crate::ui::menu::script_mod(vm);
}

/// Convenience for the requesters: raise a menu anchored to a widget's own
/// drawn rect.
pub fn open_menu(cx: &mut Cx, owner: LiveId, items: Vec<MenuItem>, anchor: Rect, place: MenuPlace) {
    cx.action(FabUiAction::OpenMenu {
        owner,
        items,
        anchor,
        place,
    });
}

/// The pick for `owner`, if this actions pass carried one.
pub fn menu_picked(actions: &Actions, owner: LiveId) -> Option<LiveId> {
    for a in ui_actions(actions) {
        if let FabUiAction::MenuPicked { owner: o, id } = a {
            if *o == owner {
                return Some(*id);
            }
        }
    }
    None
}

/// The anchor rect of a dropdown press carrying `tag`, if this pass had one.
pub fn dropdown_clicked(actions: &Actions, tag: LiveId) -> Option<Rect> {
    for a in ui_actions(actions) {
        if let FabUiAction::DropdownClicked { tag: t, anchor } = a {
            if *t == tag {
                return Some(*anchor);
            }
        }
    }
    None
}

/// The pie pick for `owner`.
pub fn pie_picked(actions: &Actions, owner: LiveId) -> Option<LiveId> {
    for a in ui_actions(actions) {
        if let FabUiAction::PiePicked { owner: o, id } = a {
            if *o == owner {
                return Some(*id);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: LiveId = live_id!(popup_a);
    const B: LiveId = live_id!(popup_b);
    const C: LiveId = live_id!(popup_c);

    /// The shell's popup state and its buttons, wired the way `FabMenuLayer`
    /// and `FabDropdownButton` wire them: the layer is the only writer, each
    /// button only ever mirrors the broadcast of one pass.
    struct Sim {
        popup: OpenPopup,
        /// (owner, its button's rect, the button's mirrored `open` flag).
        buttons: Vec<(LiveId, Rect, bool)>,
    }

    impl Sim {
        fn new(owners: &[LiveId]) -> Self {
            let buttons = owners
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let rect = Rect {
                        pos: dvec2(100.0 * i as f64, 0.0),
                        size: dvec2(80.0, 20.0),
                    };
                    (*o, rect, false)
                })
                .collect();
            Sim {
                popup: OpenPopup::default(),
                buttons,
            }
        }

        fn broadcast(&self, changes: Vec<PopupChange>) -> Vec<FabUiAction> {
            changes
                .into_iter()
                .map(|c| match c {
                    PopupChange::Closed(owner) => FabUiAction::MenuClosed { owner },
                    PopupChange::Opened(owner) => FabUiAction::MenuOpened { owner },
                })
                .collect()
        }

        /// One actions pass reaches every button; the re-presses it caused
        /// come back out, exactly like `DropdownClicked` requests would.
        fn deliver(&mut self, pass: &[FabUiAction]) -> Vec<LiveId> {
            let mut repress = Vec::new();
            for (owner, rect, open) in self.buttons.iter_mut() {
                // `was_open` is the state before the pass: a click-away's
                // MenuClosed lands in the same pass as its MenuClickAway.
                let was_open = *open;
                if let Some(next) = open_after(pass.iter(), *owner) {
                    *open = next;
                }
                for a in pass {
                    if let FabUiAction::MenuClickAway { at } = a {
                        if !was_open && rect.contains(*at) {
                            repress.push(*owner);
                        }
                    }
                }
            }
            repress
        }

        /// The layer opens `owner`'s menu (a host answered a press).
        fn open(&mut self, owner: LiveId) {
            let changes = self.popup.set(Some(owner));
            let pass = self.broadcast(changes);
            let repress = self.deliver(&pass);
            assert!(repress.is_empty());
        }

        /// The layer closes without a pointer (Escape, a pick, focus loss).
        fn close(&mut self) {
            let changes = self.popup.set(None);
            let pass = self.broadcast(changes);
            self.deliver(&pass);
        }

        /// A press at `at`: with a menu up it is the click-away the layer
        /// closes on and reports; without one it is an ordinary press on
        /// whichever button sits there.
        fn press(&mut self, at: Vec2d) {
            if self.popup.owner().is_some() {
                let changes = self.popup.set(None);
                let mut pass = self.broadcast(changes);
                pass.push(FabUiAction::MenuClickAway { at });
                for owner in self.deliver(&pass) {
                    self.open(owner);
                }
            } else if let Some((owner, _, _)) = self
                .buttons
                .iter()
                .find(|(_, r, _)| r.contains(at))
                .copied()
            {
                self.open(owner);
            }
        }

        fn open_buttons(&self) -> Vec<LiveId> {
            self.buttons
                .iter()
                .filter(|(_, _, open)| *open)
                .map(|(o, _, _)| *o)
                .collect()
        }

        fn center(&self, owner: LiveId) -> Vec2d {
            let (_, r, _) = self.buttons.iter().find(|(o, _, _)| *o == owner).unwrap();
            dvec2(r.pos.x + r.size.x * 0.5, r.pos.y + r.size.y * 0.5)
        }

        /// The invariant: the buttons that look open are exactly the owner
        /// of the menu that is up (none, when the open menu has no button —
        /// a context menu).
        fn check(&self) {
            let want: Vec<LiveId> = self
                .popup
                .owner()
                .into_iter()
                .filter(|o| self.buttons.iter().any(|(b, _, _)| b == o))
                .collect();
            assert_eq!(self.open_buttons(), want);
        }
    }

    #[test]
    fn set_reports_close_before_open_and_nothing_for_no_change() {
        let mut p = OpenPopup::default();
        assert_eq!(p.set(None), vec![]);
        assert_eq!(p.set(Some(A)), vec![PopupChange::Opened(A)]);
        assert_eq!(p.set(Some(A)), vec![]);
        assert_eq!(
            p.set(Some(B)),
            vec![PopupChange::Closed(A), PopupChange::Opened(B)]
        );
        assert_eq!(p.owner(), Some(B));
        assert_eq!(p.set(None), vec![PopupChange::Closed(B)]);
        assert_eq!(p.owner(), None);
    }

    #[test]
    fn open_after_folds_one_pass_in_order() {
        let none: Vec<FabUiAction> = vec![];
        assert_eq!(open_after(none.iter(), A), None);
        let opened = [FabUiAction::MenuOpened { owner: A }];
        assert_eq!(open_after(opened.iter(), A), Some(true));
        assert_eq!(open_after(opened.iter(), B), Some(false));
        let replaced = [
            FabUiAction::MenuClosed { owner: A },
            FabUiAction::MenuOpened { owner: B },
        ];
        assert_eq!(open_after(replaced.iter(), A), Some(false));
        assert_eq!(open_after(replaced.iter(), B), Some(true));
        let closed = [FabUiAction::MenuClosed { owner: B }];
        assert_eq!(open_after(closed.iter(), B), Some(false));
        // Somebody else's close cannot leave mine open: only one is ever up.
        assert_eq!(open_after(closed.iter(), A), Some(false));
        let unrelated = [FabUiAction::MenuClickAway {
            at: dvec2(0.0, 0.0),
        }];
        assert_eq!(open_after(unrelated.iter(), A), None);
    }

    #[test]
    fn opening_another_dropdown_moves_the_open_look_on_one_click() {
        let mut sim = Sim::new(&[A, B, C]);
        sim.check();
        sim.press(sim.center(A));
        assert_eq!(sim.open_buttons(), vec![A]);
        sim.check();
        // The press on B is a click-away for A's menu and B's own press.
        sim.press(sim.center(B));
        assert_eq!(sim.open_buttons(), vec![B]);
        sim.check();
        sim.press(sim.center(C));
        assert_eq!(sim.open_buttons(), vec![C]);
        sim.check();
    }

    #[test]
    fn pressing_the_open_button_toggles_it_closed() {
        let mut sim = Sim::new(&[A, B]);
        sim.press(sim.center(A));
        assert_eq!(sim.open_buttons(), vec![A]);
        sim.press(sim.center(A));
        assert_eq!(sim.open_buttons(), vec![]);
        sim.check();
        // And it opens again on the next press, like any idle button.
        sim.press(sim.center(A));
        assert_eq!(sim.open_buttons(), vec![A]);
        sim.check();
    }

    #[test]
    fn escape_pick_and_focus_loss_leave_no_button_open() {
        let mut sim = Sim::new(&[A, B]);
        for _ in 0..3 {
            sim.press(sim.center(B));
            assert_eq!(sim.open_buttons(), vec![B]);
            sim.close();
            assert_eq!(sim.open_buttons(), vec![]);
            sim.check();
        }
    }

    #[test]
    fn a_click_on_empty_space_closes_without_opening_anything() {
        let mut sim = Sim::new(&[A, B]);
        sim.press(sim.center(A));
        sim.press(dvec2(500.0, 300.0));
        assert_eq!(sim.open_buttons(), vec![]);
        sim.check();
        // Idle: a click on nothing stays nothing.
        sim.press(dvec2(500.0, 300.0));
        assert_eq!(sim.open_buttons(), vec![]);
        sim.check();
    }

    #[test]
    fn a_menu_raised_without_a_button_still_clears_the_buttons() {
        // A context menu (right-click in the viewport) replaces A's menu; A
        // must go idle even though no button owns the new one.
        let mut sim = Sim::new(&[A, B]);
        sim.press(sim.center(A));
        sim.open(C);
        assert_eq!(sim.open_buttons(), vec![]);
        sim.check();
        sim.press(sim.center(B));
        assert_eq!(sim.open_buttons(), vec![B]);
        sim.check();
    }
}
