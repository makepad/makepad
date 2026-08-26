//! The menu engine: one overlay layer that draws every dropdown, flyout and
//! right-click menu in the app, with real hover tracking, real keyboard
//! navigation and real submenus.
//!
//! It lives at the very end of the shell's overlay stack — after every panel,
//! before the `TipLayer` — for the same reason `TipLayer` does: an overlay
//! draw list floats over every area and splitter, and being last in the tree
//! means the layer wins hover against whatever sits underneath it.
//!
//! Anyone can raise a menu without owning one. A control emits
//! [`crate::ui::popover::FabUiAction::OpenMenu`] with the item list and the
//! anchor rect it measured from its own drawn area (never mid-pass turtle
//! state — the overlay-layer law), and reads `MenuPicked` back out of the next
//! actions pass. That is the whole contract; the vocabulary
//! (`MenuItem` / `MenuIcon` / `MenuPlace`) lives in `popover.rs` so lane D's
//! header dropdowns and this layer speak the same language.
//!
//! **Why the layer draws its own rows instead of instantiating row widgets.**
//! A menu has to answer two questions per pointer move — *which row is the
//! pointer on* and *does that row open a flyout* — and both answers must agree
//! with what is on screen to the pixel. Owning the geometry (`level_rect`,
//! `row_rect`) makes hit testing and drawing the same arithmetic, which is
//! what makes hover-into-submenu and arrow-key navigation behave. It is also
//! the `TipLayer` / `FabPieLayer` idiom already proven in this app.
//!
//! **Marks are shader-drawn, not SVG, on purpose (temporary).** Opening a menu
//! that instantiates `Icon` widgets currently corrupts the shared text atlas
//! (every label in the app garbles); the fix is landing in the draw layer.
//! Until then [`MENU_ICONS`] is `false` and the check / radio / flyout marks
//! come out of the row shader — the same way the stock `CheckBox` draws its
//! tick. Flip the flag when the atlas fix lands and the SVG glyph column takes
//! over (the icon column is already wired: [`MenuIcon`] carries the slot).

use crate::ui::popover::{FabUiAction, MenuIcon, MenuItem, MenuPlace, OpenPopup, PopupChange};
use makepad_widgets::*;

/// SVG glyphs in menu rows. `false` until the shared-atlas fix lands in the
/// draw layer (see the module doc); the shader marks stand in meanwhile.
pub const MENU_ICONS: bool = false;

/// Row metrics. The layer sizes itself from the strings — text is not measured
/// until it is drawn, so drawing and hit testing share this arithmetic and
/// never disagree.
const ROW_H: f64 = 22.0;
const SEP_H: f64 = 5.0;
const MENU_PAD: f64 = 4.0;
const MENU_MIN_W: f64 = 150.0;
/// Left column that carries the check / radio mark.
const MARK_COL: f64 = 24.0;
const RIGHT_PAD: f64 = 10.0;
const ARROW_COL: f64 = 14.0;
const CHAR_W: f64 = 5.9;
const SHORT_CHAR_W: f64 = 5.2;
/// Gap between the shortcut column and the label.
const SHORT_GAP: f64 = 18.0;

/// Keyboard navigation that the menu layer cannot answer on its own: the
/// menubar owns "the menu to the left / right of this one".
#[derive(Debug)]
pub enum MenuNav {
    /// Left / Right pressed at the root level of `owner`'s menu.
    Cycle { owner: LiveId, forward: bool },
}

/// The Left/Right request for `owner`, if this actions pass carried one.
pub fn menu_cycle(actions: &Actions, owner: LiveId) -> Option<bool> {
    for a in actions.iter() {
        if let Some(MenuNav::Cycle { owner: o, forward }) = a.downcast_ref::<MenuNav>() {
            if *o == owner {
                return Some(*forward);
            }
        }
    }
    None
}

// ===========================================================================
// Geometry (drawing and hit testing read the same functions)
// ===========================================================================

fn item_height(item: &MenuItem) -> f64 {
    if item.separator {
        SEP_H
    } else {
        ROW_H
    }
}

/// The bubble size a list of items needs.
pub fn measure(items: &[MenuItem]) -> Vec2d {
    let mut w: f64 = MENU_MIN_W;
    let mut h = MENU_PAD * 2.0;
    for it in items {
        h += item_height(it);
        if it.separator {
            continue;
        }
        let label = it.label.chars().count() as f64 * CHAR_W;
        let short = if it.shortcut.is_empty() {
            0.0
        } else {
            it.shortcut.chars().count() as f64 * SHORT_CHAR_W + SHORT_GAP
        };
        let arrow = if it.submenu.is_empty() { 0.0 } else { ARROW_COL };
        w = w.max(MARK_COL + label + short + arrow + RIGHT_PAD);
    }
    dvec2(w.ceil(), h)
}

/// One open menu. Level 0 is the root; every further level is a flyout.
struct Level {
    owner: LiveId,
    items: Vec<MenuItem>,
    rect: Rect,
    /// Highlighted row (index into `items`), pointer- or keyboard-driven.
    hi: Option<usize>,
    /// Per-row hover amount, 0..1, eased over `anim_fast` (100 ms).
    hover_t: Vec<f32>,
    /// Row currently held down.
    press: Option<usize>,
    /// Row of the *parent* level that opened this flyout.
    from_row: Option<usize>,
}

impl Level {
    fn row_rect(&self, i: usize) -> Rect {
        let mut y = self.rect.pos.y + MENU_PAD;
        for it in self.items.iter().take(i) {
            y += item_height(it);
        }
        Rect {
            pos: dvec2(self.rect.pos.x + MENU_PAD, y),
            size: dvec2(
                (self.rect.size.x - MENU_PAD * 2.0).max(0.0),
                item_height(&self.items[i]),
            ),
        }
    }

    fn row_at(&self, p: Vec2d) -> Option<usize> {
        if !self.rect.contains(p) {
            return None;
        }
        (0..self.items.len()).find(|i| self.row_rect(*i).contains(p))
    }

    fn selectable(&self, i: usize) -> bool {
        self.items
            .get(i)
            .map_or(false, |it| !it.separator && it.enabled)
    }

    /// Next selectable row from `from` in direction `dir` (+1 / −1), wrapping.
    fn step(&self, from: Option<usize>, dir: isize) -> Option<usize> {
        let n = self.items.len();
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
}

/// Clamp a bubble of `size` into `win`.
fn clamp_into(mut pos: Vec2d, size: Vec2d, win: Rect) -> Vec2d {
    let max_x = (win.pos.x + win.size.x - size.x - 4.0).max(win.pos.x + 4.0);
    let max_y = (win.pos.y + win.size.y - size.y - 4.0).max(win.pos.y + 4.0);
    pos.x = pos.x.clamp(win.pos.x + 4.0, max_x);
    pos.y = pos.y.clamp(win.pos.y + 4.0, max_y);
    pos
}

// ===========================================================================
// The layer
// ===========================================================================

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    set_type_default() do #(DrawMenuRow::script_shader(vm)){
        ..mod.draw.DrawQuad

        hover: 0.0
        down: 0.0
        mark: 0.0
        arrow: 0.0
        disabled: 0.0

        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let cy = self.rect_size.y * 0.5
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, fab.radius)
            let hot = self.hover * (1.0 - self.disabled)
            sdf.fill(vec4(fab.color_menu_row_hover.xyz, hot).mix(vec4(fab.color_accent_dim.xyz, hot), self.down))
            if self.mark > 1.5 {
                sdf.circle(12.0, cy, 2.6)
                sdf.fill(fab.color_text)
            } else {
                if self.mark > 0.5 {
                    sdf.move_to(7.5, cy + 0.5)
                    sdf.line_to(10.5, cy + 3.5)
                    sdf.line_to(16.0, cy - 3.5)
                    sdf.stroke(fab.color_ok, 1.5)
                }
            }
            if self.arrow > 0.5 {
                let ax = self.rect_size.x - 12.0
                sdf.move_to(ax, cy - 3.5)
                sdf.line_to(ax + 3.5, cy)
                sdf.line_to(ax, cy + 3.5)
                sdf.stroke(fab.color_text_dim, 1.2)
            }
            return sdf.result
        }
    }

    mod.widgets.FabMenuLayerBase = #(FabMenuLayer::register_widget(vm))
    mod.widgets.FabMenuLayer = set_type_default() do mod.widgets.FabMenuLayerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius_lg)
                sdf.fill_keep(fab.color_popover)
                sdf.stroke(fab.color_popover_border, 1.0)
                return sdf.result
            }
        }
        draw_row +: { }
        draw_sep +: {
            color: fab.color_menu_sep
        }
        draw_label +: {
            color: fab.color_text
            ink_centered: true
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }
        draw_shortcut +: {
            color: fab.color_text_muted
            ink_centered: true
            text_style: theme.font_regular{
                font_size: fab.font_size_small
            }
        }
    }
}

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

#[derive(Script, ScriptHook, Widget)]
pub struct FabMenuLayer {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    draw_list: DrawList2d,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_row: DrawMenuRow,
    #[live]
    draw_sep: DrawColor,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_shortcut: DrawText,
    #[walk]
    walk: Walk,
    #[rust]
    area: Area,
    #[rust]
    levels: Vec<Level>,
    /// Window rect from the last draw — placement clamps against it.
    #[rust]
    window: Rect,
    /// True while this layer holds `cx.sweep_lock`, which is what stops a
    /// click on a menu row from *also* reaching the control underneath it.
    #[rust]
    locked: bool,
    /// A click-away closed the menu on the press; eat the matching release so
    /// the dismissing click does not act on what was under the menu.
    #[rust]
    swallow_up: bool,
    #[rust]
    opened_at: f64,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_t: f64,
    /// The single source of truth for "which popup is open" — every root
    /// open/close funnels through [`Self::set_open`] and is broadcast for
    /// the popup buttons and the menu bar to mirror.
    #[rust]
    popup: OpenPopup,
}

impl FabMenuLayer {
    fn owner(&self) -> LiveId {
        self.levels.first().map(|l| l.owner).unwrap_or(LiveId(0))
    }

    /// Laid-out width of one line, in layout points. Draw-pass only.
    fn text_width(&mut self, cx: &mut Cx2d, small: bool, text: &str) -> f64 {
        if text.is_empty() {
            return 0.0;
        }
        let draw = if small {
            &self.draw_shortcut
        } else {
            &self.draw_label
        };
        draw.prepare_single_line_run(cx, text)
            .map(|r| r.width_in_lpxs as f64)
            .unwrap_or_else(|| text.chars().count() as f64 * CHAR_W)
    }

    fn redraw_menus(&mut self, cx: &mut Cx) {
        self.draw_list.redraw(cx);
        self.area.redraw(cx);
    }

    /// Take the pointer for as long as a menu is up.
    ///
    /// Without this an open menu is only *drawn* on top: the press that picks
    /// a row travels on down the tree and every widget under the bubble that
    /// hit-tests normally answers it too — picking "Select All" also opened
    /// the viewport header's mode dropdown that happened to sit under the
    /// row. `sweep_lock` makes `Event::hits` answer nothing outside this
    /// layer, which is exactly the modal grab a menu needs.
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

    /// Drop every level. Each close path funnels through here, so the
    /// broadcast can never miss one. Returns the owner that was up.
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

    /// Every change of "which popup is open" goes through the tracker and
    /// out as `MenuClosed` / `MenuOpened`, close before open, for the popup
    /// buttons and the menu bar to mirror. The modal grab also kept every
    /// hover-out from arriving while a menu was up, so any transition ends
    /// by clearing hover/press visuals tree-wide; the next pointer move
    /// re-hovers whatever really is under the pointer.
    fn set_open(&mut self, cx: &mut Cx, next: Option<LiveId>) {
        let changes = self.popup.set(next);
        if changes.is_empty() {
            return;
        }
        for change in changes {
            match change {
                PopupChange::Closed(owner) => cx.action(FabUiAction::MenuClosed { owner }),
                PopupChange::Opened(owner) => cx.action(FabUiAction::MenuOpened { owner }),
            }
        }
        cx.clear_all_hovers();
    }

    fn open_root(&mut self, cx: &mut Cx, owner: LiveId, items: Vec<MenuItem>, anchor: Rect, place: MenuPlace) {
        if items.is_empty() {
            return;
        }
        let win = if self.window.size.x > 1.0 {
            self.window
        } else {
            Rect {
                pos: dvec2(0.0, 0.0),
                size: cx.default_window_size(),
            }
        };
        let size = measure(&items);
        let pos = match place {
            MenuPlace::Below => dvec2(anchor.pos.x, anchor.pos.y + anchor.size.y + 2.0),
            MenuPlace::BelowRight => dvec2(
                anchor.pos.x + anchor.size.x - size.x,
                anchor.pos.y + anchor.size.y + 2.0,
            ),
            MenuPlace::At => anchor.pos,
        };
        let pos = clamp_into(pos, size, win);
        self.levels.clear();
        let n = items.len();
        self.levels.push(Level {
            owner,
            items,
            rect: Rect { pos, size },
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

    /// Open the flyout of row `row` of level `level` (no-op when it is already
    /// the level above, so hovering inside an open flyout is stable).
    fn open_flyout(&mut self, cx: &mut Cx, level: usize, row: usize) {
        if self.levels.len() > level + 1 && self.levels[level + 1].from_row == Some(row) {
            return;
        }
        let (items, anchor, owner) = {
            let l = &self.levels[level];
            let Some(it) = l.items.get(row) else { return };
            if it.submenu.is_empty() {
                self.levels.truncate(level + 1);
                self.redraw_menus(cx);
                return;
            }
            (it.submenu.clone(), l.row_rect(row), l.owner)
        };
        let size = measure(&items);
        let parent = self.levels[level].rect;
        let mut pos = dvec2(parent.pos.x + parent.size.x - 3.0, anchor.pos.y - MENU_PAD);
        // Flip to the left when the flyout would leave the window.
        if pos.x + size.x > self.window.pos.x + self.window.size.x - 4.0 {
            pos.x = parent.pos.x - size.x + 3.0;
        }
        let pos = clamp_into(pos, size, self.window);
        self.levels.truncate(level + 1);
        let n = items.len();
        self.levels.push(Level {
            owner,
            items,
            rect: Rect { pos, size },
            hi: None,
            hover_t: vec![0.0; n],
            press: None,
            from_row: Some(row),
        });
        self.redraw_menus(cx);
    }

    /// Pointer landed on `row` of `level`: highlight it, open or drop flyouts.
    fn point_at(&mut self, cx: &mut Cx, level: usize, row: Option<usize>) {
        let changed = self.levels[level].hi != row;
        self.levels[level].hi = row;
        match row {
            Some(r) if self.levels[level].items.get(r).map_or(false, |i| !i.submenu.is_empty() && i.enabled) => {
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

    /// Fire row `row` of the deepest level, or step into its flyout.
    fn activate(&mut self, cx: &mut Cx, level: usize, row: usize) -> bool {
        let Some(it) = self.levels[level].items.get(row).cloned() else {
            return false;
        };
        if it.separator || !it.enabled {
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
        self.levels.clear();
        self.unlock_input(cx);
        self.redraw_menus(cx);
        self.set_open(cx, None);
        cx.action(FabUiAction::MenuPicked { owner, id: it.id });
        true
    }

    fn tick_hover(&mut self, cx: &mut Cx) {
        let now = cx.seconds_since_app_start();
        let dt = (now - self.last_t).clamp(0.0, 0.05);
        self.last_t = now;
        let speed = (dt / 0.10) as f32;
        let mut moving = false;
        for li in 0..self.levels.len() {
            let n = self.levels[li].items.len();
            if self.levels[li].hover_t.len() != n {
                self.levels[li].hover_t.resize(n, 0.0);
            }
            let hi = self.levels[li].hi;
            for i in 0..n {
                let target = if hi == Some(i) && self.levels[li].selectable(i) {
                    1.0
                } else {
                    0.0
                };
                let t = self.levels[li].hover_t[i];
                if (t - target).abs() > 0.01 {
                    self.levels[li].hover_t[i] = if target > t {
                        (t + speed).min(target)
                    } else {
                        (t - speed).max(target)
                    };
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
        for li in (0..self.levels.len()).rev() {
            if let Some(row) = self.levels[li].row_at(p) {
                return Some((li, row));
            }
        }
        None
    }

    fn inside_any(&self, p: Vec2d) -> Option<usize> {
        (0..self.levels.len()).rev().find(|li| self.levels[*li].rect.contains(p))
    }

    fn key_down(&mut self, cx: &mut Cx, key: KeyCode) -> bool {
        let last = self.levels.len() - 1;
        match key {
            KeyCode::Escape => {
                if self.close(cx).is_some() {
                    self.unlock_input(cx);
                }
                true
            }
            KeyCode::ArrowDown | KeyCode::ArrowUp => {
                let dir = if key == KeyCode::ArrowDown { 1 } else { -1 };
                let from = self.levels[last].hi;
                self.levels[last].hi = self.levels[last].step(from, dir);
                self.levels.truncate(last + 1);
                self.redraw_menus(cx);
                true
            }
            KeyCode::ArrowRight => {
                let hi = self.levels[last].hi;
                let has_sub = hi
                    .and_then(|i| self.levels[last].items.get(i))
                    .map_or(false, |it| !it.submenu.is_empty() && it.enabled);
                if has_sub {
                    self.activate(cx, last, hi.unwrap());
                } else {
                    let owner = self.owner();
                    cx.action(MenuNav::Cycle { owner, forward: true });
                }
                true
            }
            KeyCode::ArrowLeft => {
                if last > 0 {
                    self.levels.truncate(last);
                    self.redraw_menus(cx);
                } else {
                    let owner = self.owner();
                    cx.action(MenuNav::Cycle { owner, forward: false });
                }
                true
            }
            KeyCode::ReturnKey | KeyCode::NumpadEnter => {
                if let Some(i) = self.levels[last].hi {
                    self.activate(cx, last, i);
                }
                true
            }
            _ => false,
        }
    }
}

impl Widget for FabMenuLayer {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let window = cx.turtle().rect();
        cx.end_turtle_with_area(&mut self.area);
        self.window = window;
        if self.levels.is_empty() {
            return DrawStep::done();
        }

        // The open-time size came from a character-count estimate, because
        // text cannot be measured outside a draw pass. Now that we are in one,
        // measure for real and widen — a shortcut column that overlaps its
        // label is the classic estimated-menu bug, and no estimate survives
        // every string.
        for li in 0..self.levels.len() {
            let mut need = MENU_MIN_W;
            for i in 0..self.levels[li].items.len() {
                let item = self.levels[li].items[i].clone();
                if item.separator {
                    continue;
                }
                let label_w = self.text_width(cx, false, &item.label);
                let short_w = if item.shortcut.is_empty() {
                    0.0
                } else {
                    self.text_width(cx, true, &item.shortcut) + SHORT_GAP
                };
                let arrow_w = if item.submenu.is_empty() { 0.0 } else { ARROW_COL };
                need = need.max(MARK_COL + label_w + short_w + arrow_w + RIGHT_PAD);
            }
            let need = need.ceil();
            if (need - self.levels[li].rect.size.x).abs() > 0.5 {
                self.levels[li].rect.size.x = need;
                let size = self.levels[li].rect.size;
                let pos = self.levels[li].rect.pos;
                self.levels[li].rect.pos = clamp_into(pos, size, window);
            }
        }

        self.draw_list.begin_overlay_reuse(cx);
        let pass = cx.current_pass_size();
        cx.begin_root_turtle(pass, Layout::flow_down());

        let count = self.levels.len();
        for li in 0..count {
            let rect = self.levels[li].rect;
            let items = self.levels[li].items.clone();
            self.draw_bg.draw_abs(cx, rect);
            for (i, item) in items.iter().enumerate() {
                let r = self.levels[li].row_rect(i);
                if item.separator {
                    self.draw_sep.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(r.pos.x + 4.0, r.pos.y + (SEP_H - 1.0) * 0.5),
                            size: dvec2((r.size.x - 8.0).max(0.0), 1.0),
                        },
                    );
                    continue;
                }
                let ht = self.levels[li]
                    .hover_t
                    .get(i)
                    .copied()
                    .unwrap_or(0.0);
                self.draw_row.hover = ht;
                self.draw_row.down = if self.levels[li].press == Some(i) && item.enabled {
                    1.0
                } else {
                    0.0
                };
                self.draw_row.disabled = if item.enabled { 0.0 } else { 1.0 };
                self.draw_row.mark = if MENU_ICONS {
                    0.0
                } else {
                    match item.icon {
                        MenuIcon::Check => 1.0,
                        MenuIcon::Dot => 2.0,
                        _ => 0.0,
                    }
                };
                self.draw_row.arrow = if item.submenu.is_empty() { 0.0 } else { 1.0 };
                self.draw_row.draw_abs(cx, r);

                let short_w = self.text_width(cx, true, &item.shortcut);
                let arrow_w = if item.submenu.is_empty() { 0.0 } else { ARROW_COL };
                let text_w = (r.size.x - MARK_COL - RIGHT_PAD - short_w - arrow_w).max(8.0);
                self.draw_label.color = if item.enabled {
                    vec4(0.902, 0.902, 0.902, 1.0)
                } else {
                    vec4(0.44, 0.44, 0.44, 1.0)
                };
                self.draw_label.draw_walk(
                    cx,
                    Walk::abs_rect(Rect {
                        pos: dvec2(r.pos.x + MARK_COL, r.pos.y),
                        size: dvec2(text_w, r.size.y),
                    }),
                    Align { x: 0.0, y: 0.5 },
                    &item.label,
                );
                if !item.shortcut.is_empty() {
                    self.draw_shortcut.draw_walk(
                        cx,
                        Walk::abs_rect(Rect {
                            pos: dvec2(r.pos.x + r.size.x - RIGHT_PAD - arrow_w - short_w, r.pos.y),
                            size: dvec2(short_w, r.size.y),
                        }),
                        Align { x: 1.0, y: 0.5 },
                        &item.shortcut,
                    );
                }
            }
        }

        cx.end_pass_sized_turtle_with_shift(self.area, dvec2(0.0, 0.0) - window.pos);
        self.draw_list.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() && !self.levels.is_empty() {
            self.tick_hover(cx);
        }
        if let Event::Actions(actions) = event {
            let mut request = None;
            for a in actions.iter() {
                if let Some(FabUiAction::OpenMenu {
                    owner,
                    items,
                    anchor,
                    place,
                }) = a.downcast_ref::<FabUiAction>()
                {
                    request = Some((*owner, items.clone(), *anchor, *place));
                }
            }
            if let Some((owner, items, anchor, place)) = request {
                self.open_root(cx, owner, items, anchor, place);
            }
        }
        // The release that belongs to a dismissing press: eat it, then drop
        // the grab — unless the press landed on another menu-bar button and a
        // fresh menu is already up.
        if let Event::MouseUp(_) = event {
            if self.swallow_up {
                self.swallow_up = false;
                if self.levels.is_empty() {
                    self.unlock_input(cx);
                }
            }
        }
        // A menu cannot outlive its window's focus — and if a dismissing
        // press was still waiting for its release, that release is never
        // coming, so the grab must not wait for it either.
        if matches!(
            event,
            Event::WindowLostFocus(_) | Event::Pause | Event::Background
        ) {
            self.swallow_up = false;
            self.close(cx);
            self.unlock_input(cx);
        }
        if self.levels.is_empty() {
            return;
        }
        match event {
            Event::MouseMove(e) => {
                if let Some((li, row)) = self.hit(e.abs) {
                    self.point_at(cx, li, Some(row));
                } else if let Some(li) = self.inside_any(e.abs) {
                    self.point_at(cx, li, None);
                }
                self.tick_hover(cx);
            }
            Event::MouseDown(e) => {
                if let Some((li, row)) = self.hit(e.abs) {
                    self.point_at(cx, li, Some(row));
                    if self.levels[li].selectable(row) {
                        self.levels[li].press = Some(row);
                        self.redraw_menus(cx);
                    }
                } else if self.inside_any(e.abs).is_none() {
                    // Click-away. `MenuClickAway` tells the popup buttons
                    // where the press landed, so the one under it re-opens on
                    // this same press (the menubar does the equivalent from
                    // its own raw mouse tracking); that request arrives in
                    // the following actions pass and wins, which is why the
                    // grab is held until the release rather than dropped
                    // here.
                    if self.close(cx).is_some() {
                        self.swallow_up = true;
                        cx.action(FabUiAction::MenuClickAway { at: e.abs });
                    }
                }
            }
            Event::MouseUp(e) => {
                for level in &mut self.levels {
                    level.press = None;
                }
                if let Some((li, row)) = self.hit(e.abs) {
                    self.activate(cx, li, row);
                } else {
                    self.redraw_menus(cx);
                }
            }
            Event::KeyDown(ke) => {
                self.key_down(cx, ke.key_code);
                self.tick_hover(cx);
            }
            _ => {}
        }
    }
}
