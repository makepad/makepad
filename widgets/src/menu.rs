//! The menu engine: one overlay layer that draws every dropdown, flyout and
//! context menu in an app, with real hover tracking, real keyboard
//! navigation and real submenus.
//!
//! An app declares ONE `MenuLayer` — as the last child of a window body,
//! so it draws over every panel — and then nobody else has to own a
//! menu. A control raises one by emitting [`MenuAction::Open`] with the rows
//! it wants and the anchor rect it measured from its own drawn area, and
//! reads [`MenuAction::Picked`] back out of the next actions pass. That is
//! the whole contract, and it is why a dropdown, a context menu, a split
//! button and a toolbar's overflow can all be menus without any of them
//! containing menu code.
//!
//! **Why the layer draws its own rows instead of instantiating row
//! widgets.** A menu has to answer two questions on every pointer move —
//! which row is the pointer on, and does that row open a flyout — and both
//! answers must agree with what is on screen to the pixel. Owning the
//! geometry ([`Level::row_rect`] and its callers) makes hit testing and
//! drawing the same arithmetic, which is what makes hover-into-submenu and
//! arrow-key navigation behave. Instantiating a widget per row also costs a
//! tree rebuild every time a menu opens, on the one interaction where
//! latency is most visible.
//!
//! **Why the layer takes the pointer.** While a menu is up it holds
//! `cx.sweep_lock`, so `Event::hits` answers nothing outside it. Without
//! that grab an open menu is only DRAWN on top: the press that picks a row
//! travels on down the tree, and every widget under the bubble that hit
//! tests normally answers it too — picking "Select all" would also press
//! whatever button happened to sit under that row. The lock nests, so a
//! menu raised from inside a dialog gives the dialog its lock back when it
//! closes.
//!
//! **What a click-away does.** The dismissing press is swallowed, and the
//! layer says where it landed with [`MenuAction::ClickAway`]. A control that
//! sits under that point can then treat the press as its own, so a dropdown
//! beside an open menu opens on that one click instead of needing a second.
//! Without the report every dismissing click would be silently eaten, which
//! is the difference between a menu bar that walks and one that stutters.

use crate::{
    makepad_derive_widget::*,
    makepad_draw::*,
    overlay_place::{claim_escape, place_overlay, PlaceAlign, PlaceRequest, Placement, Side},
    widget::*,
};

/// The mark in a row's leading column: what the row says about itself
/// before its label is read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MenuMark {
    #[default]
    None,
    /// This is on, and can be turned off on its own.
    Check,
    /// This is the one chosen out of a set.
    Radio,
}

/// One row of a menu. `id` is what comes back in [`MenuAction::Picked`].
///
/// A row is data, not a widget: a menu is built, shown and thrown away
/// between two pointer events, and a list of these costs nothing to build.
#[derive(Clone, Debug)]
pub struct MenuRow {
    pub id: LiveId,
    pub label: String,
    /// The keys that do the same thing, drawn in the trailing column. The
    /// menu never binds these; it only says what they are.
    pub shortcut: String,
    pub mark: MenuMark,
    pub enabled: bool,
    /// A rule instead of a row; `id` and `label` are ignored.
    pub separator: bool,
    /// A quiet heading instead of a row: not selectable, no mark column.
    pub section: bool,
    /// Drawn in the error role, for the row that destroys something.
    pub danger: bool,
    /// Flyout rows. A non-empty submenu means this row opens instead of
    /// firing.
    pub submenu: Vec<MenuRow>,
}

impl MenuRow {
    pub fn new(id: LiveId, label: &str) -> Self {
        MenuRow {
            id,
            label: label.to_string(),
            shortcut: String::new(),
            mark: MenuMark::None,
            enabled: true,
            separator: false,
            section: false,
            danger: false,
            submenu: Vec::new(),
        }
    }

    /// A rule between groups of rows.
    pub fn separator() -> Self {
        let mut row = MenuRow::new(LiveId(0), "");
        row.separator = true;
        row
    }

    /// A quiet heading over the rows that follow it.
    pub fn section(label: &str) -> Self {
        let mut row = MenuRow::new(LiveId(0), label);
        row.section = true;
        row.enabled = false;
        row
    }

    pub fn key(mut self, shortcut: &str) -> Self {
        self.shortcut = shortcut.to_string();
        self
    }

    pub fn checked(mut self, on: bool) -> Self {
        self.mark = if on { MenuMark::Check } else { MenuMark::None };
        self
    }

    pub fn radio(mut self, on: bool) -> Self {
        self.mark = if on { MenuMark::Radio } else { MenuMark::None };
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    pub fn danger(mut self, on: bool) -> Self {
        self.danger = on;
        self
    }

    pub fn submenu(mut self, rows: Vec<MenuRow>) -> Self {
        self.submenu = rows;
        self
    }
}

/// Where a menu hangs off its anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Script, ScriptHook)]
#[repr(u32)]
pub enum MenuPlace {
    /// Menu-bar style: below the anchor, left edges aligned.
    #[pick]
    Below = 0,
    /// Popover style: below the anchor, right edges aligned.
    BelowRight = 1,
    /// Context style: at the pointer, growing right and down.
    At = 2,
}

impl MenuPlace {
    fn placement(self) -> Placement {
        match self {
            MenuPlace::Below => Placement::new(Side::Bottom, PlaceAlign::Start),
            MenuPlace::BelowRight => Placement::new(Side::Bottom, PlaceAlign::End),
            MenuPlace::At => Placement::new(Side::Bottom, PlaceAlign::Start),
        }
    }
}

/// The menu bus. Every control that raises a menu speaks exactly this.
#[derive(Clone, Debug)]
pub enum MenuAction {
    /// Raise a menu. `owner` is any id the requester picks, so it can tell
    /// its own menus apart when the pick comes back.
    Open {
        owner: LiveId,
        rows: Vec<MenuRow>,
        anchor: Rect,
        place: MenuPlace,
    },
    /// Raise a menu that STAYS open when a row is chosen: a set of
    /// switches is not finished after one press.
    OpenSet {
        owner: LiveId,
        rows: Vec<MenuRow>,
        anchor: Rect,
        place: MenuPlace,
    },
    /// Replace the rows of the menu `owner` has open, leaving it open and
    /// leaving the highlight where it is. This is how a set of switches
    /// shows a mark changing under the pointer. A flyout that is open shows
    /// its row's new submenu, so a switch inside one changes under the
    /// pointer as well.
    Update { owner: LiveId, rows: Vec<MenuRow> },
    /// A row was chosen. The `Closed` for that menu is in the same pass.
    Picked { owner: LiveId, id: LiveId },
    /// A menu is up for `owner`. When it replaced another, that one's
    /// `Closed` comes first in the same pass.
    Opened { owner: LiveId },
    /// `owner`'s menu is down, whatever took it down: a pick, Escape, a
    /// press outside, another menu opening over it, the window losing
    /// focus. A control that lights up while its menu is open mirrors this
    /// pair rather than keeping a flag of its own, because a flag has to be
    /// cleared on every one of those paths and the one that gets forgotten
    /// is the highlight that lingers.
    Closed { owner: LiveId },
    /// The press that dismissed a menu landed at `at`, outside every
    /// bubble. The grab hid that press from whatever sits there, so a
    /// control under `at` may treat it as its own.
    ClickAway { at: Vec2d },
    /// Left or Right pressed at the root level: the menu bar owns "the menu
    /// beside this one", so the layer asks rather than guesses.
    Cycle { owner: LiveId, forward: bool },
}

/// Every menu action in a pass.
pub fn menu_actions(actions: &Actions) -> impl Iterator<Item = &MenuAction> {
    actions.iter().filter_map(|a| a.downcast_ref::<MenuAction>())
}

/// The row `owner` picked this pass, if it picked one.
pub fn menu_picked(actions: &Actions, owner: LiveId) -> Option<LiveId> {
    menu_actions(actions).find_map(|a| match a {
        MenuAction::Picked { owner: o, id } if *o == owner => Some(*id),
        _ => None,
    })
}

/// The Left/Right request for `owner`, if this pass carried one.
pub fn menu_cycle(actions: &Actions, owner: LiveId) -> Option<bool> {
    menu_actions(actions).find_map(|a| match a {
        MenuAction::Cycle { owner: o, forward } if *o == owner => Some(*forward),
        _ => None,
    })
}

/// Which menu is open — the one fact every control that raises menus
/// shares.
///
/// The layer routes every change through [`OpenMenu::set`] and broadcasts
/// the result, close before open. "Exactly one control looks open, and only
/// while its menu is up" is then a property of the bus rather than of each
/// control's luck with the events a modal grab lets through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenMenu {
    owner: Option<LiveId>,
}

/// One step of the broadcast, in the order it must be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuChange {
    Closed(LiveId),
    Opened(LiveId),
}

impl OpenMenu {
    pub fn owner(&self) -> Option<LiveId> {
        self.owner
    }

    pub fn is_open(&self, owner: LiveId) -> bool {
        self.owner == Some(owner)
    }

    /// Move to `next` and answer what to broadcast: the close of the
    /// previous menu before the open of the new one, nothing when nothing
    /// changed.
    pub fn set(&mut self, next: Option<LiveId>) -> Vec<MenuChange> {
        if self.owner == next {
            return Vec::new();
        }
        let mut changes = Vec::new();
        if let Some(previous) = self.owner {
            changes.push(MenuChange::Closed(previous));
        }
        if let Some(owner) = next {
            changes.push(MenuChange::Opened(owner));
        }
        self.owner = next;
        changes
    }
}

/// Row metrics. The layer sizes itself from the strings, and text cannot be
/// measured outside a draw pass, so an open estimates from character counts
/// and the next draw widens to the truth.
const ROW_H: f64 = 22.0;
const SEP_H: f64 = 5.0;
const SECTION_H: f64 = 18.0;
const MENU_PAD: f64 = 4.0;
const MENU_MIN_W: f64 = 150.0;
/// The leading column that carries the check or radio mark.
const MARK_COL: f64 = 24.0;
const RIGHT_PAD: f64 = 10.0;
const ARROW_COL: f64 = 14.0;
const CHAR_W: f64 = 5.9;
const SHORT_CHAR_W: f64 = 5.2;
/// Gap between a label and the shortcut column.
const SHORT_GAP: f64 = 18.0;
/// How long a row takes to light up.
const HOVER_SECS: f64 = 0.10;

fn row_height(row: &MenuRow) -> f64 {
    if row.separator {
        SEP_H
    } else if row.section {
        SECTION_H
    } else {
        ROW_H
    }
}

/// The bubble size a list of rows needs, estimated from character counts.
pub fn measure_rows(rows: &[MenuRow]) -> Vec2d {
    let mut w: f64 = MENU_MIN_W;
    let mut h = MENU_PAD * 2.0;
    for row in rows {
        h += row_height(row);
        if row.separator {
            continue;
        }
        let label = row.label.chars().count() as f64 * CHAR_W;
        let short = if row.shortcut.is_empty() {
            0.0
        } else {
            row.shortcut.chars().count() as f64 * SHORT_CHAR_W + SHORT_GAP
        };
        let arrow = if row.submenu.is_empty() { 0.0 } else { ARROW_COL };
        w = w.max(MARK_COL + label + short + arrow + RIGHT_PAD);
    }
    dvec2(w.ceil(), h)
}

/// One open menu. Level 0 is the root; every further level is a flyout.
struct Level {
    owner: LiveId,
    rows: Vec<MenuRow>,
    rect: Rect,
    /// Highlighted row, pointer- or keyboard-driven.
    hi: Option<usize>,
    /// Per-row hover amount, 0..1.
    hover_t: Vec<f32>,
    /// Row currently held down.
    press: Option<usize>,
    /// Row of the PARENT level that opened this flyout.
    from_row: Option<usize>,
}

/// Each open flyout shows its parent row's submenu again, after the root's
/// rows were replaced: a switch picked inside one then shows its new mark.
/// A flyout whose rows no longer line up is left as it was, and so is
/// everything past it.
fn refresh_flyouts(levels: &mut [Level]) {
    for index in 1..levels.len() {
        let Some(from) = levels[index].from_row else { break };
        let Some(submenu) = levels[index - 1].rows.get(from).map(|row| row.submenu.clone()) else { break };
        if submenu.len() != levels[index].rows.len() {
            break;
        }
        levels[index].rows = submenu;
    }
}

impl Level {
    fn row_rect(&self, i: usize) -> Rect {
        let mut y = self.rect.pos.y + MENU_PAD;
        for row in self.rows.iter().take(i) {
            y += row_height(row);
        }
        Rect {
            pos: dvec2(self.rect.pos.x + MENU_PAD, y),
            size: dvec2(
                (self.rect.size.x - MENU_PAD * 2.0).max(0.0),
                row_height(&self.rows[i]),
            ),
        }
    }

    fn row_at(&self, p: Vec2d) -> Option<usize> {
        if !self.rect.contains(p) {
            return None;
        }
        (0..self.rows.len()).find(|i| self.row_rect(*i).contains(p))
    }

    fn selectable(&self, i: usize) -> bool {
        self.rows
            .get(i)
            .is_some_and(|row| !row.separator && !row.section && row.enabled)
    }

    /// The next selectable row from `from` in direction `dir`, wrapping.
    fn step(&self, from: Option<usize>, dir: isize) -> Option<usize> {
        let n = self.rows.len();
        if n == 0 {
            return None;
        }
        let start = match from {
            Some(i) => i as isize,
            None => {
                if dir > 0 {
                    -1
                } else {
                    n as isize
                }
            }
        };
        for k in 1..=n as isize {
            let i = (start + dir * k).rem_euclid(n as isize) as usize;
            if self.selectable(i) {
                return Some(i);
            }
        }
        None
    }

    /// The next selectable row whose label starts with `c`, searched from
    /// after the highlighted one so repeated presses walk the matches.
    fn typeahead(&self, c: char) -> Option<usize> {
        let n = self.rows.len();
        let start = self.hi.map(|i| i as isize).unwrap_or(-1);
        let c = c.to_lowercase().next()?;
        (1..=n as isize).find_map(|k| {
            let i = (start + k).rem_euclid(n as isize) as usize;
            let starts = self.rows[i]
                .label
                .chars()
                .next()
                .and_then(|first| first.to_lowercase().next())
                == Some(c);
            (self.selectable(i) && starts).then_some(i)
        })
    }
}

/// The letter a key stands for, for typeahead only: menus are walked by
/// first letter, and the platform's key codes carry no character of their
/// own. Anything that is not a letter or a digit answers `None`, so the
/// keys a menu already means something by are never swallowed.
/// Whether the mouse press that is still held is the one that raised a menu
/// anchored on `anchor`.
///
/// Press the title, drag down the rows, release on one: that is a single
/// gesture belonging to the control that opened the menu, and the menu is
/// the far end of it. The press that raised it landed on the control the
/// menu is anchored to, which is what tells it apart from a press some other
/// control took before the menu ever went up. A context menu's anchor is the
/// point itself, and `Rect::contains` is inclusive, so the press that raised
/// one is inside its own anchor too.
fn raised_by_the_press(anchor: Rect, held_press: Option<DVec2>) -> bool {
    held_press.map_or(false, |at| anchor.contains(at))
}

/// Whether an open menu may follow the pointer: light the row under it, arm
/// a row on a press, choose one on the release.
///
/// `mouse_held_outside` is [`CxFingers::is_mouse_held_outside`] asked with
/// this layer's own area. It is the app-wide rule: a control that is dragged
/// continuously locks the pointer on its press, and nothing else may take a
/// hover, a focus or a press-like state from that pointer until the release.
/// A menu reads raw events, so `hits`' half of that rule never runs for it
/// and it has to ask the question itself.
///
/// The menu stands down, it does not close: DISMISSAL never asks this, so a
/// menu left up while a control elsewhere is dragged is still closed by a
/// press outside it. And a TOUCH press answers `mouse_held_outside` false by
/// design, so a finger walking a menu is untouched.
fn menu_follows_pointer(mouse_held_outside: bool, raised_by_the_press: bool) -> bool {
    !mouse_held_outside || raised_by_the_press
}

fn typed_letter(key: KeyCode) -> Option<char> {
    let c = match key {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        KeyCode::Key0 => '0',
        KeyCode::Key1 => '1',
        KeyCode::Key2 => '2',
        KeyCode::Key3 => '3',
        KeyCode::Key4 => '4',
        KeyCode::Key5 => '5',
        KeyCode::Key6 => '6',
        KeyCode::Key7 => '7',
        KeyCode::Key8 => '8',
        KeyCode::Key9 => '9',
        _ => return None,
    };
    Some(c)
}

/// One row's background, its mark and its flyout arrow.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMenuRow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    down: f32,
    /// 0 none, 1 check, 2 radio dot.
    #[live]
    mark: f32,
    #[live]
    arrow: f32,
    #[live]
    disabled: f32,
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Registered before the `use` below: a block's `use` is a snapshot of
    // what exists when it runs.
    mod.widgets.MenuPlace = set_type_default() do #(MenuPlace::script_api(vm))
    mod.widgets.splat(mod.widgets.MenuPlace)

    use mod.widgets.*

    mod.widgets.DrawMenuRowBase = #(DrawMenuRow::script_component(vm))
    set_type_default() do #(DrawMenuRow::script_shader(vm)){
        ..mod.draw.DrawQuad

        hover: 0.0
        down: 0.0
        mark: 0.0
        arrow: 0.0
        disabled: 0.0

        /** the row's own colours; the layer sets nothing but the state */
        color_hover: uniform(theme.color_primary_container)
        color_down: uniform(theme.color_primary)
        color_mark: uniform(theme.color_on_surface)
        color_arrow: uniform(theme.color_on_surface_variant)
        radius: uniform(theme.radius_s)

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let cy = self.rect_size.y * 0.5
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.radius)
            let hot = self.hover * (1.0 - self.disabled)
            sdf.fill(mix(vec4(self.color_hover.xyz, hot), vec4(self.color_down.xyz, hot), self.down))
            if self.mark > 1.5 {
                sdf.circle(12.0, cy, 2.6)
                sdf.fill(self.color_mark)
            } else {
                if self.mark > 0.5 {
                    sdf.move_to(7.5, cy + 0.5)
                    sdf.line_to(10.5, cy + 3.5)
                    sdf.line_to(16.0, cy - 3.5)
                    sdf.stroke(self.color_mark, 1.5)
                }
            }
            if self.arrow > 0.5 {
                let ax = self.rect_size.x - 12.0
                sdf.move_to(ax, cy - 3.5)
                sdf.line_to(ax + 3.5, cy)
                sdf.line_to(ax, cy + 3.5)
                sdf.stroke(self.color_arrow, 1.2)
            }
            return sdf.result
        }
    }

    mod.widgets.DrawMenuBgBase = #(DrawMenuBg::script_component(vm))
    set_type_default() do #(DrawMenuBg::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.MenuLayerBase = #(MenuLayer::register_widget(vm))
    /** The one layer that draws every menu in an app. Declare it once. */
    mod.widgets.MenuLayer = set_type_default() do mod.widgets.MenuLayerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            /** the bubble's fill */
            color: theme.color_surface_container_high
            /** the bubble's edge */
            border_color: theme.color_outline
            border_size: 1.0
            radius: theme.radius_l
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, self.border_size)
                return sdf.result
            }
        }
        draw_row +: { }
        draw_sep +: {
            color: theme.color_outline_variant
        }
        draw_label +: {
            color: theme.color_on_surface
            ink_centered: true
            text_style: theme.font_body_m
        }
        draw_section +: {
            color: theme.color_on_surface_variant
            ink_centered: true
            text_style: theme.font_label_s
        }
        draw_shortcut +: {
            color: theme.color_on_surface_variant
            ink_centered: true
            text_style: theme.font_body_s
        }
        /** the ink of a row that destroys something */
        color_danger: theme.color_error
        /** the ink of a row that cannot be chosen */
        color_disabled: theme.color_text_disabled
    }
}

/// The bubble's own quad: fill, edge and corner.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMenuBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_color: Vec4f,
    #[live]
    border_size: f32,
    #[live]
    radius: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct MenuLayer {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_list: DrawList2d,
    #[redraw]
    #[live]
    draw_bg: DrawMenuBg,
    #[live]
    draw_row: DrawMenuRow,
    #[live]
    draw_sep: DrawColor,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_section: DrawText,
    #[live]
    draw_shortcut: DrawText,
    #[live]
    color_danger: Vec4f,
    #[live]
    color_disabled: Vec4f,
    #[walk]
    walk: Walk,

    #[rust]
    area: Area,
    #[rust]
    levels: Vec<Level>,
    /// The room a menu has to fit in, from the last draw: the whole pass,
    /// NOT this layer's own rect. A layer declared as the last child of a
    /// page gets whatever space is left at the bottom of it, and clamping
    /// menus to that strip would drag every one of them down there.
    #[rust]
    window: Rect,
    /// Where this layer sits, which is the origin its overlay pass is
    /// shifted from. Only ever that.
    #[rust]
    origin: DVec2,
    /// True while this layer holds the sweep lock.
    #[rust]
    locked: bool,
    /// A click-away closed the menu on the press; the matching release is
    /// eaten so the dismissing click does not act on what was under the
    /// menu.
    #[rust]
    swallow_up: bool,
    /// Where the mouse went down, while it is still down. The layer reads
    /// raw presses, so it sees every press without a capture of its own,
    /// which is how a menu raised mid-press can tell whose press it is.
    #[rust]
    held_press: Option<DVec2>,
    /// The press still held is the one that raised the open menu, so this
    /// menu is the far end of that gesture and goes on tracking it. See
    /// [`raised_by_the_press`].
    #[rust]
    raised_by_held_press: bool,
    #[rust]
    opened_at: f64,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_t: f64,
    /// The single source of truth for which menu is open.
    #[rust]
    open: OpenMenu,
    /// Whether the open menu stays up when a row is chosen: a set of
    /// switches is not finished after one press, a list of commands is.
    #[rust]
    stay_open: bool,
}

impl MenuLayer {
    fn owner(&self) -> LiveId {
        self.levels.first().map(|l| l.owner).unwrap_or(LiveId(0))
    }

    /// Whether any menu is up.
    pub fn is_open(&self) -> bool {
        !self.levels.is_empty()
    }

    /// A line's laid-out width. Draw pass only.
    fn text_width(&mut self, cx: &mut Cx2d, small: bool, text: &str) -> f64 {
        if text.is_empty() {
            return 0.0;
        }
        let draw = if small { &self.draw_shortcut } else { &self.draw_label };
        draw.prepare_single_line_run(cx, text)
            .map(|r| r.width_in_lpxs as f64)
            .unwrap_or_else(|| text.chars().count() as f64 * CHAR_W)
    }

    fn redraw_menus(&mut self, cx: &mut Cx) {
        self.draw_list.redraw(cx);
        self.area.redraw(cx);
    }

    fn lock_input(&mut self, cx: &mut Cx) {
        if !self.locked {
            cx.sweep_lock(self.area);
            self.locked = true;
        }
    }

    fn unlock_input(&mut self, cx: &mut Cx) {
        if self.locked {
            cx.sweep_unlock(self.area);
            self.locked = false;
        }
    }

    /// Drop every level. Every close path funnels through here, so the
    /// broadcast can never miss one. Answers the owner that was up.
    pub fn close(&mut self, cx: &mut Cx) -> Option<LiveId> {
        if self.levels.is_empty() {
            return None;
        }
        let owner = self.owner();
        self.levels.clear();
        self.redraw_menus(cx);
        self.set_open(cx, None);
        Some(owner)
    }

    /// Every change of which menu is open goes out as `Closed` then
    /// `Opened`. The modal grab also kept every hover-out from arriving
    /// while the menu was up, so a transition ends by clearing hover and
    /// press visuals tree-wide; the next pointer move re-hovers whatever is
    /// really under the pointer.
    fn set_open(&mut self, cx: &mut Cx, next: Option<LiveId>) {
        let changes = self.open.set(next);
        if changes.is_empty() {
            return;
        }
        for change in changes {
            match change {
                MenuChange::Closed(owner) => cx.action(MenuAction::Closed { owner }),
                MenuChange::Opened(owner) => cx.action(MenuAction::Opened { owner }),
            }
        }
        cx.clear_all_hovers();
    }

    /// Raise a menu. The anchor must be a rect the caller measured from its
    /// own drawn area, never mid-pass turtle state.
    pub fn open_root(
        &mut self,
        cx: &mut Cx,
        owner: LiveId,
        rows: Vec<MenuRow>,
        anchor: Rect,
        menu_place: MenuPlace,
    ) {
        self.open_root_with(cx, owner, rows, anchor, menu_place, false)
    }

    /// Raise a menu that stays open when a row is chosen, for a set of
    /// switches rather than a list of commands.
    pub fn open_set(
        &mut self,
        cx: &mut Cx,
        owner: LiveId,
        rows: Vec<MenuRow>,
        anchor: Rect,
        menu_place: MenuPlace,
    ) {
        self.open_root_with(cx, owner, rows, anchor, menu_place, true)
    }

    /// Replace the rows of the menu that is open for `owner`, keeping it
    /// open and keeping the highlight where it is. Open flyouts take
    /// their rows from the new ones too.
    pub fn update_rows(&mut self, cx: &mut Cx, owner: LiveId, rows: Vec<MenuRow>) {
        let Some(level) = self.levels.first_mut() else {
            return;
        };
        if level.owner != owner || rows.len() != level.rows.len() {
            return;
        }
        level.rows = rows;
        refresh_flyouts(&mut self.levels);
        self.redraw_menus(cx);
    }

    fn open_root_with(
        &mut self,
        cx: &mut Cx,
        owner: LiveId,
        rows: Vec<MenuRow>,
        anchor: Rect,
        menu_place: MenuPlace,
        stay_open: bool,
    ) {
        if rows.is_empty() {
            return;
        }
        self.stay_open = stay_open;
        let bounds = if self.window.size.x > 1.0 {
            self.window
        } else {
            Rect { pos: dvec2(0.0, 0.0), size: cx.default_window_size() }
        };
        let size = measure_rows(&rows);
        // A context menu hangs off the point itself, which is an anchor of
        // no size; everything else hangs off the control that raised it.
        let anchor = match menu_place {
            MenuPlace::At => Rect { pos: anchor.pos, size: dvec2(0.0, 0.0) },
            _ => anchor,
        };
        self.raised_by_held_press = raised_by_the_press(anchor, self.held_press);
        let placed = place_overlay(&PlaceRequest {
            anchor,
            size,
            bounds,
            gap: if menu_place == MenuPlace::At { 0.0 } else { 2.0 },
            placement: menu_place.placement(),
            match_anchor_width: false,
        });
        self.levels.clear();
        let n = rows.len();
        self.levels.push(Level {
            owner,
            rows,
            rect: placed.rect,
            hi: None,
            hover_t: vec![0.0; n],
            press: None,
            from_row: None,
        });
        self.opened_at = cx.seconds_since_app_start();
        self.last_t = self.opened_at;
        self.next_frame = cx.new_next_frame();
        self.lock_input(cx);
        self.redraw_menus(cx);
        self.set_open(cx, Some(owner));
    }

    /// Open the flyout of `row` of `level`. A no-op when that flyout is
    /// already the level above, so hovering inside an open flyout is
    /// stable.
    fn open_flyout(&mut self, cx: &mut Cx, level: usize, row: usize) {
        if self.levels.len() > level + 1 && self.levels[level + 1].from_row == Some(row) {
            return;
        }
        let (rows, anchor, owner) = {
            let l = &self.levels[level];
            let Some(it) = l.rows.get(row) else { return };
            if it.submenu.is_empty() {
                self.levels.truncate(level + 1);
                self.redraw_menus(cx);
                return;
            }
            (it.submenu.clone(), l.row_rect(row), l.owner)
        };
        let size = measure_rows(&rows);
        let parent = self.levels[level].rect;
        // A flyout hangs off the parent bubble's edge, not off the row: the
        // row is inside the bubble, and a flyout that overlapped its parent
        // would cover the trail the pointer is travelling along.
        let flyout_anchor = Rect {
            pos: dvec2(parent.pos.x, anchor.pos.y - MENU_PAD),
            size: dvec2(parent.size.x - 3.0, 0.0),
        };
        let placed = place_overlay(&PlaceRequest {
            anchor: flyout_anchor,
            size,
            bounds: self.window,
            gap: 0.0,
            placement: Placement::new(Side::Right, PlaceAlign::Start),
            match_anchor_width: false,
        });
        self.levels.truncate(level + 1);
        let n = rows.len();
        self.levels.push(Level {
            owner,
            rows,
            rect: placed.rect,
            hi: None,
            hover_t: vec![0.0; n],
            press: None,
            from_row: Some(row),
        });
        self.redraw_menus(cx);
    }

    /// The pointer landed on `row` of `level`: highlight it, open or drop
    /// flyouts.
    fn point_at(&mut self, cx: &mut Cx, level: usize, row: Option<usize>) {
        let changed = self.levels[level].hi != row;
        self.levels[level].hi = row;
        match row {
            Some(r)
                if self.levels[level]
                    .rows
                    .get(r)
                    .is_some_and(|it| !it.submenu.is_empty() && it.enabled) =>
            {
                self.open_flyout(cx, level, r);
            }
            _ => {
                if self.levels.len() > level + 1 {
                    self.levels.truncate(level + 1);
                    self.redraw_menus(cx);
                }
            }
        }
        if changed {
            self.redraw_menus(cx);
        }
    }

    /// Fire `row` of `level`, or step into its flyout.
    fn activate(&mut self, cx: &mut Cx, level: usize, row: usize) -> bool {
        let Some(it) = self.levels[level].rows.get(row).cloned() else {
            return false;
        };
        if it.separator || it.section || !it.enabled {
            return false;
        }
        if !it.submenu.is_empty() {
            self.open_flyout(cx, level, row);
            let last = self.levels.len() - 1;
            self.levels[last].hi = self.levels[last].step(None, 1);
            self.redraw_menus(cx);
            return false;
        }
        let owner = self.owner();
        if self.stay_open {
            // A set of switches is not finished after one press: report the
            // row and leave the menu where it is, so the next one is one
            // press away and the mark can change under the pointer.
            self.levels[level].press = None;
            self.redraw_menus(cx);
            cx.action(MenuAction::Picked { owner, id: it.id });
            return true;
        }
        self.levels.clear();
        self.unlock_input(cx);
        self.redraw_menus(cx);
        self.set_open(cx, None);
        cx.action(MenuAction::Picked { owner, id: it.id });
        true
    }

    fn tick_hover(&mut self, cx: &mut Cx) {
        let now = cx.seconds_since_app_start();
        let dt = (now - self.last_t).clamp(0.0, 0.05);
        self.last_t = now;
        let speed = (dt / HOVER_SECS) as f32;
        let mut moving = false;
        for li in 0..self.levels.len() {
            let n = self.levels[li].rows.len();
            if self.levels[li].hover_t.len() != n {
                self.levels[li].hover_t.resize(n, 0.0);
            }
            let hi = self.levels[li].hi;
            for i in 0..n {
                let target = if hi == Some(i) && self.levels[li].selectable(i) { 1.0 } else { 0.0 };
                let t = self.levels[li].hover_t[i];
                if (t - target).abs() > 0.01 {
                    self.levels[li].hover_t[i] =
                        if target > t { (t + speed).min(target) } else { (t - speed).max(target) };
                    moving = true;
                } else {
                    self.levels[li].hover_t[i] = target;
                }
            }
        }
        let opening = now - self.opened_at < 0.15;
        if moving || opening {
            self.next_frame = cx.new_next_frame();
            self.redraw_menus(cx);
        }
    }

    fn hit(&self, p: Vec2d) -> Option<(usize, usize)> {
        (0..self.levels.len())
            .rev()
            .find_map(|li| self.levels[li].row_at(p).map(|row| (li, row)))
    }

    fn inside_any(&self, p: Vec2d) -> Option<usize> {
        (0..self.levels.len()).rev().find(|li| self.levels[*li].rect.contains(p))
    }

    fn key_down(&mut self, cx: &mut Cx, ke: &KeyEvent) -> bool {
        let last = self.levels.len() - 1;
        match ke.key_code {
            KeyCode::Escape => {
                // One press closes one overlay: the menu asks for it, and
                // leaves it alone if something inside it got there first.
                if !claim_escape(cx) {
                    return true;
                }
                if self.close(cx).is_some() {
                    self.unlock_input(cx);
                }
                true
            }
            KeyCode::ArrowDown | KeyCode::ArrowUp => {
                let dir = if ke.key_code == KeyCode::ArrowDown { 1 } else { -1 };
                let from = self.levels[last].hi;
                self.levels[last].hi = self.levels[last].step(from, dir);
                self.levels.truncate(last + 1);
                self.redraw_menus(cx);
                true
            }
            KeyCode::ArrowRight => {
                let hi = self.levels[last].hi;
                let has_sub = hi
                    .and_then(|i| self.levels[last].rows.get(i))
                    .is_some_and(|it| !it.submenu.is_empty() && it.enabled);
                if has_sub {
                    self.activate(cx, last, hi.unwrap());
                } else {
                    let owner = self.owner();
                    cx.action(MenuAction::Cycle { owner, forward: true });
                }
                true
            }
            KeyCode::ArrowLeft => {
                if last > 0 {
                    self.levels.truncate(last);
                    self.redraw_menus(cx);
                } else {
                    let owner = self.owner();
                    cx.action(MenuAction::Cycle { owner, forward: false });
                }
                true
            }
            KeyCode::Home => {
                self.levels[last].hi = self.levels[last].step(None, 1);
                self.redraw_menus(cx);
                true
            }
            KeyCode::End => {
                self.levels[last].hi = self.levels[last].step(None, -1);
                self.redraw_menus(cx);
                true
            }
            KeyCode::ReturnKey | KeyCode::NumpadEnter => {
                if let Some(i) = self.levels[last].hi {
                    self.activate(cx, last, i);
                }
                true
            }
            _ => {
                // Typeahead: a letter walks the rows that start with it, so
                // a long menu is reachable without the arrow keys.
                let Some(c) = typed_letter(ke.key_code) else {
                    return false;
                };
                if let Some(i) = self.levels[last].typeahead(c) {
                    self.levels[last].hi = Some(i);
                    self.levels.truncate(last + 1);
                    self.redraw_menus(cx);
                }
                true
            }
        }
    }
}

impl Widget for MenuLayer {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let origin = cx.turtle().rect().pos;
        cx.end_turtle_with_area(&mut self.area);
        self.origin = origin;
        let window = Rect { pos: dvec2(0.0, 0.0), size: cx.current_pass_size() };
        self.window = window;
        if self.levels.is_empty() {
            return DrawStep::done();
        }

        // The open-time size came from a character-count estimate, because
        // text cannot be measured outside a draw pass. Now that we are in
        // one, measure for real and widen: a shortcut column that overlaps
        // its label is the classic estimated-menu bug, and no estimate
        // survives every string.
        for li in 0..self.levels.len() {
            let mut need = MENU_MIN_W;
            for i in 0..self.levels[li].rows.len() {
                let row = self.levels[li].rows[i].clone();
                if row.separator {
                    continue;
                }
                let label_w = self.text_width(cx, row.section, &row.label);
                let short_w = if row.shortcut.is_empty() {
                    0.0
                } else {
                    self.text_width(cx, true, &row.shortcut) + SHORT_GAP
                };
                let arrow_w = if row.submenu.is_empty() { 0.0 } else { ARROW_COL };
                need = need.max(MARK_COL + label_w + short_w + arrow_w + RIGHT_PAD);
            }
            let need = need.ceil();
            if (need - self.levels[li].rect.size.x).abs() > 0.5 {
                let rect = self.levels[li].rect;
                let placed = place_overlay(&PlaceRequest {
                    anchor: Rect { pos: rect.pos, size: dvec2(0.0, 0.0) },
                    size: dvec2(need, rect.size.y),
                    bounds: window,
                    gap: 0.0,
                    placement: Placement::new(Side::Bottom, PlaceAlign::Start),
                    match_anchor_width: false,
                });
                self.levels[li].rect.size.x = need;
                self.levels[li].rect.pos = placed.rect.pos;
            }
        }

        self.draw_list.begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::flow_down());

        for li in 0..self.levels.len() {
            let rect = self.levels[li].rect;
            let rows = self.levels[li].rows.clone();
            self.draw_bg.draw_abs(cx, rect);
            for (i, row) in rows.iter().enumerate() {
                let r = self.levels[li].row_rect(i);
                if row.separator {
                    self.draw_sep.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(r.pos.x + 4.0, r.pos.y + (SEP_H - 1.0) * 0.5),
                            size: dvec2((r.size.x - 8.0).max(0.0), 1.0),
                        },
                    );
                    continue;
                }
                if row.section {
                    self.draw_section.draw_walk(
                        cx,
                        Walk::abs_rect(Rect {
                            pos: dvec2(r.pos.x + MARK_COL, r.pos.y),
                            size: dvec2((r.size.x - MARK_COL).max(8.0), r.size.y),
                        }),
                        Align { x: 0.0, y: 0.5 },
                        &row.label,
                    );
                    continue;
                }
                let ht = self.levels[li].hover_t.get(i).copied().unwrap_or(0.0);
                self.draw_row.hover = ht;
                self.draw_row.down =
                    if self.levels[li].press == Some(i) && row.enabled { 1.0 } else { 0.0 };
                self.draw_row.disabled = if row.enabled { 0.0 } else { 1.0 };
                self.draw_row.mark = match row.mark {
                    MenuMark::Check => 1.0,
                    MenuMark::Radio => 2.0,
                    MenuMark::None => 0.0,
                };
                self.draw_row.arrow = if row.submenu.is_empty() { 0.0 } else { 1.0 };
                self.draw_row.draw_abs(cx, r);

                let short_w = self.text_width(cx, true, &row.shortcut);
                let arrow_w = if row.submenu.is_empty() { 0.0 } else { ARROW_COL };
                let text_w = (r.size.x - MARK_COL - RIGHT_PAD - short_w - arrow_w).max(8.0);
                let rest = self.draw_label.color;
                self.draw_label.color = if !row.enabled {
                    self.color_disabled
                } else if row.danger {
                    self.color_danger
                } else {
                    rest
                };
                self.draw_label.draw_walk(
                    cx,
                    Walk::abs_rect(Rect {
                        pos: dvec2(r.pos.x + MARK_COL, r.pos.y),
                        size: dvec2(text_w, r.size.y),
                    }),
                    Align { x: 0.0, y: 0.5 },
                    &row.label,
                );
                self.draw_label.color = rest;
                if !row.shortcut.is_empty() {
                    self.draw_shortcut.draw_walk(
                        cx,
                        Walk::abs_rect(Rect {
                            pos: dvec2(r.pos.x + r.size.x - RIGHT_PAD - arrow_w - short_w, r.pos.y),
                            size: dvec2(short_w, r.size.y),
                        }),
                        Align { x: 1.0, y: 0.5 },
                        &row.shortcut,
                    );
                }
            }
        }

        cx.end_pass_sized_turtle_with_shift(self.area, dvec2(0.0, 0.0) - origin);
        self.draw_list.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        // Worked out before this event moves the pointer state on: the
        // release that ends a press-drag is still part of that gesture, and
        // the capture it belongs to is only let go of after the whole tree
        // has been handed the release.
        let follows_pointer = menu_follows_pointer(
            cx.fingers.is_mouse_held_outside(&[self.area]),
            self.raised_by_held_press,
        );
        match event {
            Event::MouseDown(me) => self.held_press = Some(me.abs),
            Event::MouseUp(_) => {
                self.held_press = None;
                self.raised_by_held_press = false;
            }
            Event::FingerCancel(c) if c.device.is_mouse() && cx.fingers.press_taken_away(c.digit_id) => {
                self.held_press = None;
                self.raised_by_held_press = false;
            }
            _ => {}
        }
        if self.next_frame.is_event(event).is_some() && !self.levels.is_empty() {
            self.tick_hover(cx);
        }
        if let Event::Actions(actions) = event {
            let mut request = None;
            let mut set_request = None;
            let mut update = None;
            for a in actions.iter() {
                match a.downcast_ref::<MenuAction>() {
                    Some(MenuAction::Open { owner, rows, anchor, place }) => {
                        request = Some((*owner, rows.clone(), *anchor, *place));
                    }
                    Some(MenuAction::OpenSet { owner, rows, anchor, place }) => {
                        set_request = Some((*owner, rows.clone(), *anchor, *place));
                    }
                    Some(MenuAction::Update { owner, rows }) => {
                        update = Some((*owner, rows.clone()));
                    }
                    _ => {}
                }
            }
            if let Some((owner, rows, anchor, menu_place)) = request {
                self.open_root(cx, owner, rows, anchor, menu_place);
            }
            if let Some((owner, rows, anchor, menu_place)) = set_request {
                self.open_set(cx, owner, rows, anchor, menu_place);
            }
            if let Some((owner, rows)) = update {
                self.update_rows(cx, owner, rows);
            }
        }
        // The release that belongs to a dismissing press: eat it, then drop
        // the grab, unless a fresh menu is already up.
        if matches!(event, Event::MouseUp(_))
            || matches!(event, Event::FingerCancel(c) if c.device.is_mouse() && cx.fingers.press_taken_away(c.digit_id))
        {
            if self.swallow_up {
                self.swallow_up = false;
                if self.levels.is_empty() {
                    self.unlock_input(cx);
                }
            }
        }
        // A menu cannot outlive its window's focus, and if a dismissing
        // press was still waiting for its release, that release is never
        // coming, so the grab must not wait for it either.
        if matches!(event, Event::WindowLostFocus(_) | Event::Pause | Event::Background) {
            self.swallow_up = false;
            self.held_press = None;
            self.raised_by_held_press = false;
            self.close(cx);
            self.unlock_input(cx);
        }
        if self.levels.is_empty() {
            return;
        }
        match event {
            Event::MouseMove(e) => {
                // The highlight is a hover, and a hover taken from a pointer
                // another control is holding is the thing the rule forbids.
                if follows_pointer {
                    if let Some((li, row)) = self.hit(e.abs) {
                        self.point_at(cx, li, Some(row));
                    } else if let Some(li) = self.inside_any(e.abs) {
                        self.point_at(cx, li, None);
                    }
                }
                self.tick_hover(cx);
            }
            Event::MouseDown(e) => {
                if let Some((li, row)) = self.hit(e.abs) {
                    // A second button pressed while a control elsewhere has
                    // the mouse arms nothing; the press is over the menu all
                    // the same, so it is not a click-away either.
                    if follows_pointer {
                        self.point_at(cx, li, Some(row));
                        if self.levels[li].selectable(row) {
                            self.levels[li].press = Some(row);
                            self.redraw_menus(cx);
                        }
                    }
                } else if self.inside_any(e.abs).is_none() {
                    // Click-away. The grab is held until the release rather
                    // than dropped here, so the control under the press can
                    // claim it from `ClickAway` in the next pass.
                    if self.close(cx).is_some() {
                        self.swallow_up = true;
                        cx.action(MenuAction::ClickAway { at: e.abs });
                    }
                }
            }
            // The mouse press itself taken away chooses no row.
            Event::FingerCancel(c) if c.device.is_mouse() && cx.fingers.press_taken_away(c.digit_id) => {
                for level in &mut self.levels {
                    level.press = None;
                }
                self.redraw_menus(cx);
            }
            Event::MouseUp(e) => {
                for level in &mut self.levels {
                    level.press = None;
                }
                // Choosing a row on a release is what a press-drag menu is
                // for, and exactly what a drag that belongs to another
                // control must never do: letting go of a fader over an open
                // menu would otherwise fire the row it happened to end on.
                match self.hit(e.abs).filter(|_| follows_pointer) {
                    Some((li, row)) => {
                        self.activate(cx, li, row);
                    }
                    None => self.redraw_menus(cx),
                }
            }
            Event::KeyDown(ke) => {
                self.key_down(cx, ke);
                self.tick_hover(cx);
            }
            _ => {}
        }
    }

    /// The label of the highlighted row, so a test can wait on where the
    /// menu is without reading pixels.
    fn text(&self) -> String {
        self.levels
            .last()
            .and_then(|l| l.hi.and_then(|i| l.rows.get(i)))
            .map(|row| row.label.clone())
            .unwrap_or_default()
    }
}

impl MenuLayerRef {
    /// Raise a menu on this layer directly, for a host that would rather
    /// call than broadcast.
    pub fn open(
        &self,
        cx: &mut Cx,
        owner: LiveId,
        rows: Vec<MenuRow>,
        anchor: Rect,
        place: MenuPlace,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open_root(cx, owner, rows, anchor, place);
        }
    }

    /// Raise a menu that stays open when a row is chosen.
    pub fn open_set(
        &self,
        cx: &mut Cx,
        owner: LiveId,
        rows: Vec<MenuRow>,
        anchor: Rect,
        place: MenuPlace,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.open_set(cx, owner, rows, anchor, place);
        }
    }

    /// Replace the rows of the menu open for `owner`, keeping it open.
    pub fn update_rows(&self, cx: &mut Cx, owner: LiveId, rows: Vec<MenuRow>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.update_rows(cx, owner, rows);
        }
    }

    /// Which menu is open, if any.
    pub fn open_owner(&self) -> Option<LiveId> {
        self.borrow().and_then(|inner| inner.open.owner())
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map(|inner| inner.is_open()).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<MenuRow> {
        vec![
            MenuRow::section("File"),
            MenuRow::new(live_id!(open), "Open").key("Ctrl+O"),
            MenuRow::new(live_id!(save), "Save").key("Ctrl+S"),
            MenuRow::separator(),
            MenuRow::new(live_id!(shut), "Close").enabled(false),
            MenuRow::new(live_id!(wipe), "Delete").danger(true),
        ]
    }

    fn level() -> Level {
        let list = rows();
        let n = list.len();
        Level {
            owner: live_id!(owner),
            rect: Rect { pos: dvec2(0.0, 0.0), size: measure_rows(&list) },
            rows: list,
            hi: None,
            hover_t: vec![0.0; n],
            press: None,
            from_row: None,
        }
    }

    /// Only rows that can be chosen take the highlight: a heading, a rule
    /// and a disabled row are all skipped, in both directions, wrapping.
    #[test]
    fn the_arrows_walk_only_the_rows_that_can_be_chosen() {
        crate::on_test_cx(|| {
        let l = level();
        assert_eq!(l.step(None, 1), Some(1), "the heading is skipped");
        assert_eq!(l.step(Some(1), 1), Some(2));
        assert_eq!(l.step(Some(2), 1), Some(5), "the rule and the disabled row are skipped");
        assert_eq!(l.step(Some(5), 1), Some(1), "and it wraps");
        assert_eq!(l.step(None, -1), Some(5), "up from nowhere is the last row");
        assert_eq!(l.step(Some(1), -1), Some(5));
        });
    }

    /// A letter walks the rows starting with it, from after the highlight,
    /// so pressing it again finds the next one rather than sticking.
    #[test]
    fn a_letter_walks_the_rows_that_start_with_it() {
        crate::on_test_cx(|| {
        let mut l = level();
        assert_eq!(l.typeahead('s'), Some(2), "Save");
        assert_eq!(l.typeahead('o'), Some(1), "Open");
        assert_eq!(l.typeahead('z'), None, "nothing starts with z");
        l.hi = Some(2);
        assert_eq!(l.typeahead('o'), Some(1), "the search wraps");
        // A heading and a disabled row never answer, whatever their letter.
        assert_eq!(l.typeahead('f'), None, "the File heading is not selectable");
        assert_eq!(l.typeahead('c'), None, "the disabled Close is not selectable");
        });
    }

    /// A switch picked inside an open flyout shows its new mark: replacing
    /// the root's rows hands the open flyout its row's new submenu, and a
    /// flyout whose rows no longer line up keeps what it had.
    #[test]
    fn replacing_the_rows_refreshes_the_open_flyout() {
        crate::on_test_cx(|| {
        let switches = |on: bool| vec![MenuRow::new(live_id!(a), "A").checked(on), MenuRow::new(live_id!(b), "B")];
        let root = |on: bool| vec![MenuRow::new(live_id!(group), "Group").submenu(switches(on)), MenuRow::new(live_id!(other), "Other")];
        let mut root_level = level();
        root_level.rows = root(false);
        let mut flyout = level();
        flyout.rows = switches(false);
        flyout.from_row = Some(0);
        let mut levels = vec![root_level, flyout];
        levels[0].rows = root(true);
        refresh_flyouts(&mut levels);
        assert_eq!(levels[1].rows[0].mark, MenuMark::Check, "the flyout shows the new mark");
        // A submenu that changed length is not forced onto the open flyout.
        levels[0].rows[0].submenu.push(MenuRow::new(live_id!(c), "C"));
        levels[0].rows[0].submenu[0].mark = MenuMark::None;
        refresh_flyouts(&mut levels);
        assert_eq!(levels[1].rows.len(), 2);
        assert_eq!(levels[1].rows[0].mark, MenuMark::Check);
        });
    }

    /// Rows are measured where they are drawn: a rule is thinner than a
    /// row, a heading shorter, and the bubble is the sum plus its padding.
    #[test]
    fn the_bubble_is_as_tall_as_the_rows_it_holds() {
        crate::on_test_cx(|| {
        let size = measure_rows(&rows());
        let expected = MENU_PAD * 2.0 + SECTION_H + ROW_H * 4.0 + SEP_H;
        assert_eq!(size.y, expected);
        assert!(size.x >= MENU_MIN_W);
        let l = level();
        assert_eq!(l.row_rect(0).size.y, SECTION_H);
        assert_eq!(l.row_rect(3).size.y, SEP_H);
        assert_eq!(l.row_rect(1).size.y, ROW_H);
        });
    }

    /// The open menu is one fact, and every change of it broadcasts the
    /// close before the open so a control cannot see two menus open at
    /// once.
    #[test]
    fn one_menu_is_open_and_every_change_says_so_in_order() {
        crate::on_test_cx(|| {
        let mut open = OpenMenu::default();
        assert_eq!(open.owner(), None);
        assert_eq!(open.set(Some(live_id!(a))), vec![MenuChange::Opened(live_id!(a))]);
        assert!(open.is_open(live_id!(a)));
        assert_eq!(open.set(Some(live_id!(a))), vec![], "nothing changed, nothing said");
        assert_eq!(
            open.set(Some(live_id!(b))),
            vec![MenuChange::Closed(live_id!(a)), MenuChange::Opened(live_id!(b))],
            "close before open"
        );
        assert_eq!(open.set(None), vec![MenuChange::Closed(live_id!(b))]);
        });
    }

    /// The app-wide rule and its one exception, in one place: a menu follows
    /// the pointer only while nothing else holds it, or while the press that
    /// raised the menu is the one holding it.
    #[test]
    fn a_menu_follows_only_a_pointer_that_is_its_own() {
        crate::on_test_cx(|| {
        assert!(menu_follows_pointer(false, false), "a free pointer is everyone's");
        assert!(!menu_follows_pointer(true, false), "another control is being dragged");
        assert!(menu_follows_pointer(true, true), "the press that raised it is still down");
        assert!(menu_follows_pointer(false, true));
        });
    }

    /// Whose press it is, read from where it landed: on the control the menu
    /// hangs off, or somewhere else entirely.
    #[test]
    fn the_press_that_raised_a_menu_is_the_one_that_landed_on_its_anchor() {
        crate::on_test_cx(|| {
        let anchor = Rect { pos: dvec2(100.0, 40.0), size: dvec2(80.0, 20.0) };
        assert!(raised_by_the_press(anchor, Some(dvec2(140.0, 50.0))));
        assert!(!raised_by_the_press(anchor, Some(dvec2(400.0, 300.0))), "a press on something else");
        assert!(!raised_by_the_press(anchor, None), "no press is being held at all");
        // A context menu hangs off the point itself, and the press that
        // raised it IS that point.
        let at = dvec2(400.0, 300.0);
        assert!(raised_by_the_press(Rect { pos: at, size: dvec2(0.0, 0.0) }, Some(at)));
        });
    }
}

/// The rule driven through the real event path, with a button holding the
/// pointer the way any control that is dragged continuously holds it.
#[cfg(test)]
mod pointer_tests {
    use super::*;
    use crate::makepad_draw::cx_draw::CxDraw;
    use std::cell::Cell;

    const SIZE: DVec2 = DVec2 { x: 800.0, y: 600.0 };

    /// A pass and a list to draw a page into, the way a window holds one.
    struct Target {
        pass: DrawPass,
        draw_list: DrawList2d,
        overlay: Overlay,
    }

    impl Target {
        fn new(cx: &mut Cx) -> Self {
            let overlay = cx.with_vm(|vm| Overlay::script_new(vm));
            Target { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx), overlay }
        }

        fn draw(&mut self, cx: &mut Cx, root: &WidgetRef) {
            self.pass.set_size(cx, SIZE);
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&self.pass, None);
            self.draw_list.begin_always(&mut cx2d);
            self.overlay.begin(&mut cx2d);
            cx2d.begin_root_turtle(SIZE, Layout::flow_down());
            root.draw_all(&mut cx2d, &mut Scope::empty());
            cx2d.end_pass_sized_turtle();
            self.overlay.end(&mut cx2d);
            self.draw_list.end(&mut cx2d);
            cx2d.end_pass(&self.pass);
        }
    }

    fn cx() -> crate::PooledCx {
        crate::checkout_test_cx()
    }

    /// A button to hold the pointer down on, and a layer to raise menus in.
    fn page(cx: &mut Cx) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = crate::script_eval!(vm, {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: Fill
                    height: Fill
                    flow: Down
                    grab := Button{text: "Grab"}
                    menus := MenuLayer{}
                }
            });
            WidgetRef::script_from_value(vm, value)
        })
    }

    fn press(abs: DVec2) -> Event {
        Event::MouseDown(MouseDownEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            handled: Cell::new(Area::Empty),
            time: 0.0,
        })
    }

    fn drag(abs: DVec2) -> Event {
        Event::MouseMove(MouseMoveEvent {
            abs,
            lock_delta: DVec2::default(),
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
            handled: Cell::new(Area::Empty),
        })
    }

    fn release(abs: DVec2) -> Event {
        Event::MouseUp(MouseUpEvent {
            abs,
            button: MouseButton::PRIMARY,
            window_id: WindowId(1, 1),
            modifiers: KeyModifiers::default(),
            time: 0.0,
        })
    }

    fn rows() -> Vec<MenuRow> {
        vec![MenuRow::new(live_id!(open), "Open"), MenuRow::new(live_id!(save), "Save")]
    }

    fn open_menu(cx: &mut Cx, root: &WidgetRef, anchor: Rect) {
        let layer = root.widget(cx, ids!(menus));
        let mut inner = layer.borrow_mut::<MenuLayer>().expect("a menu layer");
        inner.open_root(cx, live_id!(owner), rows(), anchor, MenuPlace::Below);
    }

    /// The middle of row `row` of the open menu, on screen.
    fn row_point(cx: &Cx, root: &WidgetRef, row: usize) -> DVec2 {
        let layer = root.widget(cx, ids!(menus));
        let inner = layer.borrow::<MenuLayer>().expect("a menu layer");
        inner.levels[0].row_rect(row).center()
    }

    /// The label of the row lit right now, empty when none is.
    fn lit(cx: &Cx, root: &WidgetRef) -> String {
        let layer = root.widget(cx, ids!(menus));
        let inner = layer.borrow::<MenuLayer>().expect("a menu layer");
        inner.text()
    }

    fn is_open(cx: &Cx, root: &WidgetRef) -> bool {
        let layer = root.widget(cx, ids!(menus));
        let inner = layer.borrow::<MenuLayer>().expect("a menu layer");
        inner.is_open()
    }

    /// A control that is dragged continuously holds the pointer until the
    /// release, and a menu that happens to be up takes nothing from it: no
    /// row lights under the drag, and letting go over a row does not choose
    /// it.
    #[test]
    fn a_menu_stands_down_for_a_drag_it_was_not_raised_by() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);

        let grab = root.widget(&cx, ids!(grab)).area().rect(&cx);
        root.handle_event(&mut cx, &press(grab.center()), &mut Scope::empty());
        assert!(cx.fingers.is_mouse_held_outside(&[]), "the button took the pointer");

        // Raised somewhere else entirely while that press is still down, so
        // the held press is the button's and nobody else's.
        open_menu(&mut cx, &root, Rect { pos: dvec2(500.0, 40.0), size: dvec2(80.0, 20.0) });
        let row = row_point(&cx, &root, 1);
        root.handle_event(&mut cx, &drag(row), &mut Scope::empty());
        assert_eq!(lit(&cx, &root), "", "no row lights from a pointer another control holds");
        root.handle_event(&mut cx, &release(row), &mut Scope::empty());
        assert!(is_open(&cx, &root), "and the release chooses nothing");
        });
    }

    /// The other half: a menu raised BY the press that is still held is the
    /// far end of that same gesture — press the control, drag down the rows,
    /// release on one — and goes on walking and choosing.
    #[test]
    fn a_menu_raised_by_the_held_press_still_walks_and_chooses() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);

        let grab = root.widget(&cx, ids!(grab)).area().rect(&cx);
        root.handle_event(&mut cx, &press(grab.center()), &mut Scope::empty());
        assert!(cx.fingers.is_mouse_held_outside(&[]), "the button took the pointer");

        open_menu(&mut cx, &root, grab);
        let row = row_point(&cx, &root, 1);
        root.handle_event(&mut cx, &drag(row), &mut Scope::empty());
        assert_eq!(lit(&cx, &root), "Save", "the gesture's own pointer still lights rows");
        root.handle_event(&mut cx, &release(row), &mut Scope::empty());
        assert!(!is_open(&cx, &root), "and the release chooses the row it ended on");
        });
    }

    /// Dismissal is not a gesture that stands down: a menu left up while
    /// something else is being dragged is still closed by a press outside
    /// it, or nothing could ever take it down.
    #[test]
    fn a_press_outside_still_dismisses_a_menu_while_another_control_is_dragged() {
        crate::on_test_cx(|| {
        let mut cx = cx();
        let root = page(&mut cx);
        let mut target = Target::new(&mut cx);
        target.draw(&mut cx, &root);

        let grab = root.widget(&cx, ids!(grab)).area().rect(&cx);
        root.handle_event(&mut cx, &press(grab.center()), &mut Scope::empty());
        open_menu(&mut cx, &root, Rect { pos: dvec2(500.0, 40.0), size: dvec2(80.0, 20.0) });
        assert!(is_open(&cx, &root));

        // The second button, since the first is down on the control.
        root.handle_event(&mut cx, &press(dvec2(40.0, 560.0)), &mut Scope::empty());
        assert!(!is_open(&cx, &root), "a press nowhere near the menu closes it all the same");
        });
    }
}
