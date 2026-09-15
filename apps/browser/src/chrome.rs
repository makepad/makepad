//! The browser chrome: a custom-drawn Chrome-style tab strip (favicon +
//! title tabs with a close x, a + for a new tab) and the toolbar
//! (back / forward / reload, the omnibox with search icon and bookmark star,
//! the menu button). Hard-square Omarchy look, colours from `mod.browser_theme`.

use crate::tabs::{TabId, TabSummary};
use makepad_widgets::image::DrawImage;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.TabStripBase = #(TabStrip::register_widget(vm))

    mod.widgets.TabStrip = set_type_default() do mod.widgets.TabStripBase{
        width: Fill
        height: 36

        draw_bg +: {
            color: uniform(mod.browser_theme.darker_background)
            pixel: fn() {
                return self.color
            }
        }
        draw_tab +: {
            color: uniform(mod.browser_theme.darker_background)
            pixel: fn() {
                return self.color
            }
        }
        draw_tab_hover +: {
            color: uniform(mod.browser_theme.dark_background)
            pixel: fn() {
                return self.color
            }
        }
        draw_tab_active +: {
            color: uniform(mod.browser_theme.background)
            pixel: fn() {
                return self.color
            }
        }
        draw_sep +: {
            color: uniform(mod.browser_theme.muted)
            pixel: fn() {
                return self.color
            }
        }
        draw_text +: {
            color: mod.browser_theme.foreground
            text_style: theme.font_regular{
                font_size: 9.5
            }
        }
        draw_text_dim +: {
            color: mod.browser_theme.dark_foreground
            text_style: theme.font_regular{
                font_size: 9.5
            }
        }
        draw_close +: {
            svg: crate_resource("self:resources/icons/close.svg")
            color: mod.browser_theme.dark_foreground
        }
        draw_close_active +: {
            svg: crate_resource("self:resources/icons/close.svg")
            color: mod.browser_theme.foreground
        }
        draw_plus +: {
            svg: crate_resource("self:resources/icons/plus.svg")
            color: mod.browser_theme.foreground
        }
        draw_globe +: {
            svg: crate_resource("self:resources/icons/globe.svg")
            color: mod.browser_theme.dark_foreground
        }
    }

    // A square, flat icon button for the toolbar. A `let` so this block can
    // instantiate it below (a `use mod.widgets.*` glob is a snapshot).
    let MpToolButton = ButtonFlatterIcon{
        width: 32
        height: 32
        padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
        margin: Inset{left: 0 right: 0 top: 0 bottom: 0}
        align: Align{x: 0.5 y: 0.5}
        icon_walk: Walk{width: 16 height: 16}
        draw_icon +: {
            color: mod.browser_theme.foreground
        }
        draw_bg +: {
            border_radius: 0.0
            border_size: 0.0
            color: #00000000
            color_hover: mod.browser_theme.lighter_background
            color_down: mod.browser_theme.muted
            color_focus: #00000000
        }
    }

    mod.widgets.MpToolButton = MpToolButton

    // A row of the ⋮ menu.
    mod.widgets.MpMenuItem = ButtonFlatter{
        width: Fill
        height: 30
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 14 right: 14 top: 0 bottom: 0}
        margin: Inset{left: 0 right: 0 top: 0 bottom: 0}
        draw_text +: {
            color: mod.browser_theme.foreground
            color_hover: mod.browser_theme.bright_foreground
            text_style: theme.font_regular{
                font_size: 10
            }
        }
        draw_bg +: {
            border_radius: 0.0
            border_size: 0.0
            color: #00000000
            color_hover: mod.browser_theme.lighter_background
            color_down: mod.browser_theme.muted
        }
    }

    // No `align y: 0.5` anywhere on the omnibox's ancestry: Makepad applies
    // such alignment as a deferred shift that moves walked content (text)
    // but not `draw_abs` quads — the TextInput's caret and selection would
    // end up above the field and clipped. Heights and paddings centre
    // everything explicitly instead.
    mod.widgets.MpToolbar = SolidView{
        width: Fill
        height: 40
        flow: Right
        align: Align{x: 0.0 y: 0.0}
        padding: Inset{left: 6 right: 6 top: 4 bottom: 4}
        spacing: 2
        draw_bg +: {
            color: mod.browser_theme.background
        }

        back_btn := MpToolButton{
            draw_icon.svg: crate_resource("self:resources/icons/back.svg")
        }
        forward_btn := MpToolButton{
            draw_icon.svg: crate_resource("self:resources/icons/forward.svg")
        }
        reload_btn := MpToolButton{
            draw_icon.svg: crate_resource("self:resources/icons/reload.svg")
        }

        View{width: 4 height: Fit}

        // The omnibox: a darker square well with the search glyph, the
        // text field and the bookmark star.
        omnibox_frame := SolidView{
            width: Fill
            height: 32
            flow: Right
            align: Align{x: 0.0 y: 0.0}
            padding: Inset{left: 10 right: 2 top: 0 bottom: 0}
            spacing: 6
            draw_bg +: {
                color: mod.browser_theme.darker_background
            }
            Icon{
                margin: Inset{top: 9 bottom: 0 left: 0 right: 0}
                icon_walk: Walk{width: 14 height: 14}
                draw_icon +: {
                    svg: crate_resource("self:resources/icons/search.svg")
                    color: mod.browser_theme.dark_foreground
                }
            }
            omnibox := TextInputFlat{
                width: Fill
                height: 32
                empty_text: "Search Google or type a URL"
                // The line box is 16.8pt for this font size; text reads as
                // centred by its x-height, not its full ink box, so 1pt
                // less on top puts the x-height middle on the field's
                // middle, level with the search glyph and the star.
                padding: Inset{left: 4 right: 4 top: 6.6 bottom: 8.6}
                margin: Inset{left: 0 right: 0 top: 0 bottom: 0}
                draw_bg +: {
                    border_radius: 0.0
                    border_size: 0.0
                    color: #00000000
                    color_hover: #00000000
                    color_focus: #00000000
                    color_down: #00000000
                    color_empty: #00000000
                    border_color: #00000000
                    border_color_hover: #00000000
                    border_color_focus: #00000000
                    border_color_down: #00000000
                    border_color_empty: #00000000
                }
                draw_cursor +: {
                    color: mod.browser_theme.bright_foreground
                }
                draw_text +: {
                    color: mod.browser_theme.foreground
                    color_hover: mod.browser_theme.foreground
                    color_focus: mod.browser_theme.bright_foreground
                    color_empty: mod.browser_theme.dark_foreground
                    color_empty_hover: mod.browser_theme.dark_foreground
                    text_style: theme.font_regular{
                        font_size: 10.5
                    }
                }
            }
            star_btn := MpToolButton{
                width: 28
                height: 28
                margin: Inset{top: 2 bottom: 0 left: 0 right: 0}
                icon_walk: Walk{width: 15 height: 15}
                draw_icon +: {
                    svg: crate_resource("self:resources/icons/star.svg")
                    color: mod.browser_theme.dark_foreground
                }
            }
        }

        View{width: 4 height: Fit}

        menu_btn := MpToolButton{
            draw_icon.svg: crate_resource("self:resources/icons/menu.svg")
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum TabStripAction {
    #[default]
    None,
    Activate(TabId),
    Close(TabId),
    New,
}

#[derive(Clone, Copy, Debug)]
struct TabHit {
    id: TabId,
    rect: Rect,
    close: Rect,
}

#[derive(Script, ScriptHook, Widget)]
pub struct TabStrip {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_tab: DrawQuad,
    #[live]
    draw_tab_hover: DrawQuad,
    #[live]
    draw_tab_active: DrawQuad,
    #[live]
    draw_sep: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_text_dim: DrawText,
    #[live]
    draw_close: DrawSvg,
    #[live]
    draw_close_active: DrawSvg,
    #[live]
    draw_plus: DrawSvg,
    #[live]
    draw_globe: DrawSvg,
    #[live]
    draw_favicon: DrawImage,
    #[rust]
    tabs: Vec<TabSummary>,
    #[rust]
    hits: Vec<TabHit>,
    #[rust]
    plus_rect: Rect,
    #[rust]
    hover_tab: Option<TabId>,
    #[rust]
    hover_close: bool,
    #[rust]
    hover_plus: bool,
}

impl TabStrip {
    const TAB_MAX_WIDTH: f64 = 240.0;
    const TAB_MIN_WIDTH: f64 = 56.0;
    const TOP_GAP: f64 = 6.0;
    const LEFT_PAD: f64 = 8.0;
    const PLUS_SIZE: f64 = 28.0;

    pub fn set_tabs(&mut self, cx: &mut Cx, tabs: Vec<TabSummary>) {
        self.tabs = tabs;
        self.redraw(cx);
    }

    fn hit_at(&self, pos: Vec2d) -> (Option<TabId>, bool, bool) {
        if self.plus_rect.contains(pos) {
            return (None, false, true);
        }
        for hit in &self.hits {
            if hit.rect.contains(pos) {
                return (Some(hit.id), hit.close.contains(pos), false);
            }
        }
        (None, false, false)
    }

    fn update_hover(&mut self, cx: &mut Cx, pos: Option<Vec2d>) {
        let (tab, close, plus) = pos.map(|p| self.hit_at(p)).unwrap_or((None, false, false));
        if tab != self.hover_tab || close != self.hover_close || plus != self.hover_plus {
            self.hover_tab = tab;
            self.hover_close = close;
            self.hover_plus = plus;
            self.redraw(cx);
        }
    }
}

impl Widget for TabStrip {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(fe) => {
                let (tab, close, plus) = self.hit_at(fe.abs);
                let middle = fe.mouse_button().map(|b| b.is_middle()).unwrap_or(false);
                if plus {
                    cx.widget_action(self.uid, TabStripAction::New);
                } else if let Some(id) = tab {
                    if close || middle {
                        cx.widget_action(self.uid, TabStripAction::Close(id));
                    } else {
                        cx.widget_action(self.uid, TabStripAction::Activate(id));
                    }
                }
            }
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                self.update_hover(cx, Some(fe.abs));
            }
            Hit::FingerMove(fe) => {
                self.update_hover(cx, Some(fe.abs));
            }
            Hit::FingerHoverOut(_) => {
                self.update_hover(cx, None);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.begin(cx, walk, self.layout);
        let strip = cx.turtle().rect();
        self.hits.clear();

        let count = self.tabs.len().max(1) as f64;
        let available = (strip.size.x - Self::LEFT_PAD - Self::PLUS_SIZE - 12.0).max(0.0);
        let tab_width = (available / count)
            .min(Self::TAB_MAX_WIDTH)
            .max(Self::TAB_MIN_WIDTH.min(available.max(1.0)));
        let tab_height = strip.size.y - Self::TOP_GAP;
        let mut x = strip.pos.x + Self::LEFT_PAD;
        let y = strip.pos.y + Self::TOP_GAP;

        let tabs = self.tabs.clone();
        for (i, tab) in tabs.iter().enumerate() {
            let rect = Rect {
                pos: dvec2(x, y),
                size: dvec2(tab_width, tab_height),
            };
            let hovered = self.hover_tab == Some(tab.id);
            if tab.active {
                self.draw_tab_active.draw_abs(cx, rect);
            } else if hovered {
                self.draw_tab_hover.draw_abs(cx, rect);
            } else {
                self.draw_tab.draw_abs(cx, rect);
                // Separator between neighbouring inactive tabs.
                let next_active = tabs.get(i + 1).map(|t| t.active).unwrap_or(true);
                let next_hovered = tabs
                    .get(i + 1)
                    .map(|t| self.hover_tab == Some(t.id))
                    .unwrap_or(false);
                if !next_active && !next_hovered {
                    self.draw_sep.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x + tab_width - 0.5, y + 8.0),
                            size: dvec2(1.0, tab_height - 16.0),
                        },
                    );
                }
            }

            let wide = tab_width >= 96.0;
            let show_close = tab.active || hovered || wide;
            let icon_size = 16.0;
            let icon_y = y + (tab_height - icon_size) * 0.5;
            let mut text_x = x + 10.0;
            if tab_width >= 72.0 {
                let icon_rect = Rect {
                    pos: dvec2(x + 10.0, icon_y),
                    size: dvec2(icon_size, icon_size),
                };
                match &tab.favicon {
                    Some(favicon) => {
                        self.draw_favicon.draw_vars.set_texture(0, favicon);
                        self.draw_favicon.draw_abs(cx, icon_rect);
                    }
                    None => {
                        self.draw_globe.draw_abs(cx, icon_rect);
                    }
                }
                text_x += icon_size + 8.0;
            }

            let close_size = 16.0;
            let close_rect = Rect {
                pos: dvec2(
                    x + tab_width - close_size - 8.0,
                    y + (tab_height - close_size) * 0.5,
                ),
                size: dvec2(close_size, close_size),
            };
            let text_right = if show_close {
                close_rect.pos.x - 6.0
            } else {
                x + tab_width - 8.0
            };
            let text_width = (text_right - text_x).max(0.0);
            if text_width > 4.0 {
                let title = if tab.loading && tab.title.is_empty() {
                    "Loading…".to_string()
                } else {
                    tab.title.clone()
                };
                cx.begin_turtle(
                    Walk {
                        abs_pos: Some(dvec2(text_x, y)),
                        width: Size::Fixed(text_width),
                        height: Size::Fixed(tab_height),
                        ..Walk::default()
                    },
                    Layout {
                        clip_x: true,
                        clip_y: true,
                        align: Align { x: 0.0, y: 0.5 },
                        ..Layout::default()
                    },
                );
                if tab.active {
                    self.draw_text
                        .draw_walk(cx, Walk::fit(), Align::default(), &title);
                } else {
                    self.draw_text_dim
                        .draw_walk(cx, Walk::fit(), Align::default(), &title);
                }
                cx.end_turtle();
            }

            if show_close {
                let glyph = Rect {
                    pos: close_rect.pos + dvec2(3.0, 3.0),
                    size: dvec2(close_size - 6.0, close_size - 6.0),
                };
                if tab.active || (hovered && self.hover_close) {
                    self.draw_close_active.draw_abs(cx, glyph);
                } else {
                    self.draw_close.draw_abs(cx, glyph);
                }
            }

            self.hits.push(TabHit {
                id: tab.id,
                rect,
                close: if show_close { close_rect } else { Rect::default() },
            });
            x += tab_width;
        }

        // The new-tab "+" square.
        let plus_rect = Rect {
            pos: dvec2(x + 4.0, y + (tab_height - Self::PLUS_SIZE) * 0.5),
            size: dvec2(Self::PLUS_SIZE, Self::PLUS_SIZE),
        };
        if self.hover_plus {
            self.draw_tab_hover.draw_abs(cx, plus_rect);
        }
        self.draw_plus.draw_abs(
            cx,
            Rect {
                pos: plus_rect.pos + dvec2(7.0, 7.0),
                size: dvec2(Self::PLUS_SIZE - 14.0, Self::PLUS_SIZE - 14.0),
            },
        );
        self.plus_rect = plus_rect;

        self.draw_bg.end(cx);
        DrawStep::done()
    }
}
