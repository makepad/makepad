//! The viewer's caption: the machines open in the window as tabs (a live / connecting /
//! offline dot and the short name; "+" for a desktop that is not open), the history toggle,
//! and a small popup menu (the "+" list, a tab's menu, the scrubber's Save menu). Everything is
//! drawn in the theme's colours (`theme.*` tokens) and the caption's own font.
//!
//! The widgets read raw mouse events filtered by the window they were drawn in (every window
//! of the app sees every event), and say what happened as widget actions.

use makepad_widgets::*;
use makepad_widgets::tip::TipAction;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // A rounded box (`radius` in points), the colour non-premultiplied.
    set_type_default() do #(DrawRound::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: vec4(1.0, 1.0, 1.0, 1.0)
        radius: 4.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            // `sdf.box` draws twice the radius it is given
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.radius * 0.5)
            sdf.fill(self.color)
            return sdf.result
        }
    }

    mod.widgets.DeskTabsBase = #(DeskTabs::register_widget(vm))
    mod.widgets.DeskTabs = set_type_default() do mod.widgets.DeskTabsBase {
        width: Fit height: Fill
        draw_text +: { text_style: theme.font_regular{} }
        color_text: theme.color_label_outer
        color_dim: theme.color_label_outer_off
        color_tab: theme.color_bg_highlight
        color_hover: theme.color_bg_highlight_inline
        color_live: theme.color_success
        color_connecting: theme.color_warning
        color_offline: theme.color_error
    }

    mod.widgets.CaptionToggleBase = #(CaptionToggle::register_widget(vm))
    mod.widgets.CaptionToggle = set_type_default() do mod.widgets.CaptionToggleBase {
        width: 28 height: 24
        icon +: { svg: crate_resource("self:resources/icons/history.svg") }
        color_icon: theme.color_label_outer
        color_on_icon: theme.color_on_primary
        color_on: theme.color_primary
        color_hover: theme.color_bg_highlight_inline
    }

    mod.widgets.PopMenuBase = #(PopMenu::register_widget(vm))
    mod.widgets.PopMenu = set_type_default() do mod.widgets.PopMenuBase {
        width: Fill height: Fill
        draw_text +: { text_style: theme.font_regular{} }
        color_bg: theme.color_bg_container
        color_border: theme.color_outline_variant
        color_text: theme.color_label_outer
        color_dim: theme.color_label_outer_off
        color_hover: theme.color_bg_highlight
        color_shadow: theme.color_shadow
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRound {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
    #[live]
    pub radius: f32,
}

impl DrawRound {
    pub fn fill(&mut self, cx: &mut Cx2d, r: Rect, color: Vec4f, radius: f64) {
        if color.w <= 0.0 || r.size.x <= 0.0 || r.size.y <= 0.0 {
            return;
        }
        self.color = color;
        self.radius = radius as f32;
        self.draw_abs(cx, r);
    }
}

/// `c` with its alpha scaled.
pub fn fade(c: Vec4f, a: f32) -> Vec4f {
    vec4(c.x, c.y, c.z, c.w * a)
}

/// The width of one line of `s` in `text`'s style.
pub fn text_width(text: &mut DrawText, cx: &mut Cx2d, s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    text.prepare_single_line_run(cx, s).map(|r| r.width_in_lpxs as f64).unwrap_or(0.0)
}

/// Draws `s` vertically centred in `r` from its left (`align` 0) or centred (0.5) or right (1).
pub fn text_in(text: &mut DrawText, cx: &mut Cx2d, r: Rect, s: &str, color: Vec4f, align: f64) {
    if s.is_empty() {
        return;
    }
    let w = text_width(text, cx, s);
    let run = text.layout(cx, 0.0, 0.0, None, false, Align::default(), s);
    let h = run.size_in_lpxs.height as f64;
    let y = r.pos.y + (r.size.y - h) * 0.5 + run.ink_center_offset_in_lpxs() as f64;
    let x = r.pos.x + (r.size.x - w) * align;
    let dpi = cx.current_dpi_factor();
    text.color = color;
    text.draw_abs(cx, dvec2((x * dpi).round() / dpi, (y * dpi).round() / dpi), s);
}

pub fn contains(r: Rect, p: Vec2d) -> bool {
    p.x >= r.pos.x && p.y >= r.pos.y && p.x < r.pos.x + r.size.x && p.y < r.pos.y + r.size.y
}

/// How a machine answers, for its tab's dot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabReach {
    Live,
    Connecting,
    Offline,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabInfo {
    /// The connection the tab shows.
    pub conn: usize,
    pub name: String,
    pub reach: TabReach,
    pub selected: bool,
}

#[derive(Clone, Debug, Default)]
pub enum TabsAction {
    Select(usize),
    /// A right click: the tab's menu under `Rect` (window coordinates).
    Menu(usize, Rect),
    #[default]
    None,
}

/// A tab, by its place in the strip.
type TabHit = usize;

const TAB_H: f64 = 24.0;
const TAB_PAD: f64 = 10.0;
const DOT: f64 = 7.0;

#[derive(Script, ScriptHook, Widget)]
pub struct DeskTabs {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawRound,
    #[live]
    draw_text: DrawText,
    #[live]
    color_text: Vec4f,
    #[live]
    color_dim: Vec4f,
    #[live]
    color_tab: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_live: Vec4f,
    #[live]
    color_connecting: Vec4f,
    #[live]
    color_offline: Vec4f,
    #[rust]
    tabs: Vec<TabInfo>,
    #[rust]
    hits: Vec<(Rect, TabHit)>,
    #[rust]
    strip: Rect,
    #[rust]
    hot: Option<TabHit>,
    #[rust]
    window: Option<WindowId>,
    /// Where each tab is (`label@(x,y)`), for a selftest that clicks one.
    #[rust]
    noted: String,
}

impl DeskTabs {
    pub fn set_tabs(&mut self, cx: &mut Cx, tabs: Vec<TabInfo>) {
        if tabs != self.tabs {
            self.tabs = tabs;
            self.draw_bg.redraw(cx);
        }
    }

    fn hit(&self, p: Vec2d) -> Option<TabHit> {
        self.hits.iter().find(|(r, _)| contains(*r, p)).map(|(_, h)| *h)
    }

    fn rect_of(&self, h: TabHit) -> Rect {
        self.hits.iter().find(|(_, x)| *x == h).map(|(r, _)| *r).unwrap_or_default()
    }

    fn dot_color(&self, reach: TabReach) -> Vec4f {
        match reach {
            TabReach::Live => self.color_live,
            TabReach::Connecting => self.color_connecting,
            TabReach::Offline => self.color_offline,
        }
    }
}

impl Widget for DeskTabs {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.window = cx.get_current_window_id();
        let widths: Vec<f64> = self.tabs.iter().map(|t| text_width(&mut self.draw_text, cx, &t.name)).collect();
        // a tab is the live dot and the short name, nothing else (no close glyph: Cmd+W or the
        // tab's menu closes it)
        let tab_w = |w: f64| TAB_PAD + DOT + 6.0 + w + TAB_PAD;
        let total: f64 = widths.iter().map(|w| tab_w(*w) + 2.0).sum::<f64>();
        let area = cx.walk_turtle(Walk { width: Size::Fixed(total), ..walk });
        self.strip = area;
        self.hits.clear();
        let y = area.pos.y + ((area.size.y - TAB_H) * 0.5).floor();
        let mut x = area.pos.x;
        let mut notes = Vec::new();
        for (i, (t, w)) in self.tabs.iter().zip(widths.iter()).enumerate() {
            let tw = tab_w(*w);
            let r = Rect { pos: dvec2(x, y), size: dvec2(tw, TAB_H) };
            let hot = self.hot == Some(i);
            let bg = if t.selected { self.color_tab } else if hot { self.color_hover } else { vec4(0.0, 0.0, 0.0, 0.0) };
            self.draw_bg.fill(cx, r, bg, 7.0);
            let dot = Rect { pos: dvec2(r.pos.x + TAB_PAD, r.pos.y + (TAB_H - DOT) * 0.5), size: dvec2(DOT, DOT) };
            let dc = self.dot_color(t.reach);
            self.draw_bg.fill(cx, dot, dc, DOT);
            let label = Rect { pos: dvec2(dot.pos.x + DOT + 6.0, r.pos.y), size: dvec2(*w, TAB_H) };
            let fg = if t.selected || hot { self.color_text } else { self.color_dim };
            text_in(&mut self.draw_text, cx, label, &t.name, fg, 0.0);
            self.hits.push((r, i));
            notes.push(format!("{}@({:.0},{:.0})", t.name, r.pos.x + r.size.x * 0.5, r.pos.y + r.size.y * 0.5));
            x += tw + 2.0;
        }
        let note = notes.join(" ");
        if note != self.noted {
            crate::viewer::selftest(&format!("tabs {note}"));
            self.noted = note;
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let uid = self.uid;
        let mine = |w: WindowId| Some(w) == self.window;
        match event {
            Event::MouseMove(e) if mine(e.window_id) => {
                let hot = self.hit(e.abs);
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_bg.redraw(cx);
                }
            }
            // a tab shows its machine in this window; its menu opens it in a new one (nothing
            // moves: every window lists every machine)
            Event::MouseDown(e) if mine(e.window_id) => {
                let Some(i) = self.hit(e.abs).filter(|i| *i < self.tabs.len()) else { return };
                if e.button.is_secondary() {
                    cx.widget_action(uid, TabsAction::Menu(self.tabs[i].conn, self.rect_of(i)));
                } else {
                    cx.widget_action(uid, TabsAction::Select(self.tabs[i].conn));
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum ToggleAction {
    Clicked,
    #[default]
    None,
}

/// A caption icon button with an on state (the history scrubber's toggle).
#[derive(Script, ScriptHook, Widget)]
pub struct CaptionToggle {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawRound,
    #[live]
    icon: DrawSvg,
    #[live]
    color_icon: Vec4f,
    #[live]
    color_on_icon: Vec4f,
    #[live]
    color_on: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[rust]
    pub on: bool,
    #[rust]
    hot: bool,
    #[rust]
    rect: Rect,
    #[rust]
    window: Option<WindowId>,
}

impl CaptionToggle {
    pub fn set_on(&mut self, cx: &mut Cx, on: bool) {
        if on != self.on {
            self.on = on;
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for CaptionToggle {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.window = cx.get_current_window_id();
        let r = cx.walk_turtle(walk);
        if r != self.rect {
            crate::viewer::selftest(&format!("history toggle @({:.0},{:.0})", r.pos.x + r.size.x * 0.5, r.pos.y + r.size.y * 0.5));
        }
        self.rect = r;
        let bg = if self.on { self.color_on } else if self.hot { self.color_hover } else { vec4(0.0, 0.0, 0.0, 0.0) };
        self.draw_bg.fill(cx, r, bg, 7.0);
        let s = 16.0;
        self.icon.color = if self.on { self.color_on_icon } else { self.color_icon };
        self.icon.draw_abs(cx, Rect { pos: dvec2(r.pos.x + ((r.size.x - s) * 0.5).floor(), r.pos.y + ((r.size.y - s) * 0.5).floor()), size: dvec2(s, s) });
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event {
            Event::MouseMove(e) if Some(e.window_id) == self.window => {
                let hot = contains(self.rect, e.abs);
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_bg.redraw(cx);
                    // the tooltip: the window's tip layer (TipLayer) shows it below the caption
                    if hot {
                        let key = if cfg!(target_vendor = "apple") { "\u{21e7}\u{2318}H" } else { "Ctrl+Shift+H" };
                        cx.widget_action(self.uid, TipAction::HoverIn(format!("History  {key}"), self.rect));
                    } else {
                        cx.widget_action(self.uid, TipAction::HoverOut);
                    }
                }
            }
            Event::MouseDown(e) if Some(e.window_id) == self.window && e.button.is_primary() && contains(self.rect, e.abs) => {
                cx.widget_action(self.uid, ToggleAction::Clicked);
            }
            _ => {}
        }
    }
}

/// One entry of a [`PopMenu`]: its label, its id (the caller's), whether it can be chosen,
/// and a key hint drawn dim on the right.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub label: String,
    pub id: u64,
    pub enabled: bool,
    pub hint: String,
}

impl MenuItem {
    pub fn new(label: impl Into<String>, id: u64) -> Self {
        MenuItem { label: label.into(), id, enabled: true, hint: String::new() }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub enum PopMenuAction {
    Chose(u64),
    Dismissed,
    #[default]
    None,
}

const ROW_H: f64 = 26.0;
const MENU_PAD: f64 = 6.0;

/// A small menu over the window (the body's overlay): below its anchor, or above it when
/// `above` (the scrubber's). A click outside or Escape closes it.
#[derive(Script, ScriptHook, Widget)]
pub struct PopMenu {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawRound,
    #[live]
    draw_text: DrawText,
    #[live]
    color_bg: Vec4f,
    #[live]
    color_border: Vec4f,
    #[live]
    color_text: Vec4f,
    #[live]
    color_dim: Vec4f,
    #[live]
    color_hover: Vec4f,
    #[live]
    color_shadow: Vec4f,
    #[rust]
    items: Vec<MenuItem>,
    #[rust]
    anchor: Rect,
    #[rust]
    above: bool,
    /// What the menu is for (the caller's tag), handed back with the choice.
    #[rust]
    pub tag: u64,
    #[rust]
    rows: Vec<(Rect, usize)>,
    #[rust]
    hot: Option<usize>,
    #[rust]
    window: Option<WindowId>,
    /// Where the items are, for a selftest that clicks one.
    #[rust]
    noted: String,
}

impl PopMenu {
    pub fn open(&mut self, cx: &mut Cx, tag: u64, anchor: Rect, above: bool, items: Vec<MenuItem>) {
        self.tag = tag;
        self.anchor = anchor;
        self.above = above;
        self.items = items;
        self.hot = None;
        self.draw_bg.redraw(cx);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        if !self.items.is_empty() {
            self.items.clear();
            self.rows.clear();
            self.draw_bg.redraw(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        !self.items.is_empty()
    }
}

impl Widget for PopMenu {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.window = cx.get_current_window_id();
        let area = cx.walk_turtle(walk);
        self.rows.clear();
        if self.items.is_empty() {
            return DrawStep::done();
        }
        let label_w = self.items.iter().map(|i| text_width(&mut self.draw_text, cx, &i.label)).fold(0.0, f64::max);
        let hint_w = self.items.iter().map(|i| text_width(&mut self.draw_text, cx, &i.hint)).fold(0.0, f64::max);
        let w = (label_w + if hint_w > 0.0 { hint_w + 24.0 } else { 0.0 } + 24.0).max(self.anchor.size.x).max(160.0);
        let h = self.items.len() as f64 * ROW_H + MENU_PAD * 2.0;
        let x = self.anchor.pos.x.min(area.pos.x + area.size.x - w - 8.0).max(area.pos.x + 8.0);
        let y = if self.above { self.anchor.pos.y - h - 6.0 } else { self.anchor.pos.y + self.anchor.size.y + 4.0 };
        let panel = Rect { pos: dvec2(x, y.max(area.pos.y + 4.0)), size: dvec2(w, h) };
        self.draw_bg.fill(cx, Rect { pos: panel.pos + dvec2(0.0, 2.0), size: panel.size + dvec2(0.0, 2.0) }, fade(self.color_shadow, 0.35), 10.0);
        self.draw_bg.fill(cx, Rect { pos: panel.pos - dvec2(1.0, 1.0), size: panel.size + dvec2(2.0, 2.0) }, self.color_border, 10.0);
        self.draw_bg.fill(cx, panel, self.color_bg, 9.0);
        let items = self.items.clone();
        for (k, item) in items.iter().enumerate() {
            let r = Rect { pos: dvec2(panel.pos.x + 4.0, panel.pos.y + MENU_PAD + k as f64 * ROW_H), size: dvec2(w - 8.0, ROW_H) };
            if self.hot == Some(k) && item.enabled {
                self.draw_bg.fill(cx, r, self.color_hover, 6.0);
            }
            let fg = if item.enabled { self.color_text } else { self.color_dim };
            text_in(&mut self.draw_text, cx, Rect { pos: r.pos + dvec2(10.0, 0.0), size: r.size }, &item.label, fg, 0.0);
            text_in(&mut self.draw_text, cx, Rect { pos: r.pos, size: r.size - dvec2(10.0, 0.0) }, &item.hint, self.color_dim, 1.0);
            self.rows.push((r, k));
        }
        let note = self.rows.iter().map(|(r, k)| format!("{}@({:.0},{:.0})", self.items[*k].label, r.pos.x + r.size.x * 0.5, r.pos.y + r.size.y * 0.5)).collect::<Vec<_>>().join(" | ");
        if note != self.noted {
            crate::viewer::selftest(&format!("menu {note}"));
            self.noted = note;
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.items.is_empty() {
            return;
        }
        let uid = self.uid;
        match event {
            Event::MouseMove(e) if Some(e.window_id) == self.window => {
                let hot = self.rows.iter().find(|(r, _)| contains(*r, e.abs)).map(|(_, k)| *k);
                if hot != self.hot {
                    self.hot = hot;
                    self.draw_bg.redraw(cx);
                }
            }
            // (the press that opened the menu was handled before its action opened it)
            Event::MouseDown(e) if Some(e.window_id) == self.window => {
                let chosen = self.rows.iter().find(|(r, _)| contains(*r, e.abs)).map(|(_, k)| *k);
                match chosen {
                    Some(k) if self.items[k].enabled => {
                        let id = self.items[k].id;
                        self.close(cx);
                        cx.widget_action(uid, PopMenuAction::Chose(id));
                    }
                    Some(_) => {}
                    None => {
                        self.close(cx);
                        cx.widget_action(uid, PopMenuAction::Dismissed);
                    }
                }
            }
            Event::KeyDown(k) if k.key_code == KeyCode::Escape => {
                self.close(cx);
                cx.widget_action(uid, PopMenuAction::Dismissed);
            }
            _ => {}
        }
    }
}
