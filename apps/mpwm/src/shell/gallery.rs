//! `shell/plugins/dev-gallery/` — our version.
//!
//! `mpwm --gallery` opens one window showing every ported surface with
//! fixture data, so the whole port is verifiable over `--remote` without a
//! compositor, exactly like omarchy's own `GalleryPanel.qml` (a scrolling
//! column of sections: a bold `subtitle` header, a `bodySmall` description,
//! then the live component in a demo box at `foreground` α .04 behind a 1px
//! α .10 border).
//!
//! Everything in here is LIVE, not a picture: the bar's modules open their
//! panels, the menu filters and descends, rows highlight under the pointer,
//! the notification cards count down and dismiss, and the OSD appears when
//! the audio module is scrolled.

use makepad_widgets::*;

use super::bar::{BarData, BarModule, ShellBar, ShellBarAction};
use super::menu::{MenuSkin, ShellMenu, ShellMenuAction};
use super::notifications::{fixtures, ShellNotifications};
use super::osd::{OsdShow, ShellOsd};
use super::panels::{PanelData, PanelKind, ShellPanel, ShellPanelAction};
use super::ui::{cut_top, inset, rect, DrawShellFill, Ico, ShellDraw};
use super::{alpha, CtrlState, ShellTokens};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ShellGalleryBase = #(ShellGallery::register_widget(vm))
    mod.widgets.ShellGallery = set_type_default() do mod.widgets.ShellGalleryBase {
        width: Fill
        height: Fill
        draw_bg +: {}
        d +: {}
        bar +: {}
        menu +: {}
        launcher +: {}
        keys +: {}
        osd +: {}
        notes +: {}
        panel +: {}
        bar_panel +: {}
    }
}

/// A section of the gallery: a header, a description and a demo box.
struct Section {
    title: &'static str,
    body: &'static str,
    height: f64,
}

const SECTIONS: &[Section] = &[
    Section {
        title: "Bar",
        body: "shell.json layout: left [menu, workspaces] · center [indicators, clock, keyboard-layout] · right [bluetooth, network, audio, monitor, power]. Click a module to open its panel; the accent pill marks the open one.",
        height: 26.0,
    },
    Section {
        title: "Typography",
        body: "Style.font.*, base 12px. Every token derives from the base size; makepad draws them at px * 0.75 points.",
        height: 236.0,
    },
    Section {
        title: "Controls",
        body: "Ui/Button, ToggleSwitch, PanelSlider, TextField, PanelSectionHeader, PanelSeparator, CursorSurface — flat fills at .04/.08/.18/.22 with 1px borders at .4/.25, hard corners.",
        height: 210.0,
    },
    Section {
        title: "Menu",
        body: "Super+Space. 300px card, 18px padding, 34px header, 50px rows 3px apart, 2px gradient border over a .5 scrim. Type to filter, Enter/Right to descend, Backspace/Left to go back, click a row to activate.",
        height: 430.0,
    },
    Section {
        title: "Launcher",
        body: "Super+Alt+Space — the same surface on the apps provider, wearing the [launcher] tokens: card at .95 over the .5 scrim.",
        height: 430.0,
    },
    Section {
        title: "Keybindings",
        body: "omarchy-menu-keybindings, showing OUR binds with the combos this OS answers to.",
        height: 430.0,
    },
    Section {
        title: "Panels",
        body: "Bar flyouts: clock (560 wide), audio, power, monitor (380). Card on [popups] with a 2px border, 14px padding, anchored under its module.",
        height: 900.0,
    },
    Section {
        title: "OSD",
        body: "Volume / brightness / mic. 28px glyph, 142x6 bar, bold 14px readout, centered 67px off the bottom edge, 1200ms.",
        height: 140.0,
    },
    Section {
        title: "Notifications",
        body: "380px cards, 8px apart, top-right under the bar. Summary 2 lines bold, body 3 lines at darker(text,1.15), hover pauses the accent countdown, the close button appears on hover.",
        height: 320.0,
    },
];

#[derive(Script, ScriptHook, Widget)]
pub struct ShellGallery {
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
    draw_bg: DrawShellFill,
    #[live]
    d: ShellDraw,
    #[live]
    tokens: ShellTokens,
    #[live]
    bar: ShellBar,
    #[live]
    menu: ShellMenu,
    #[live]
    launcher: ShellMenu,
    #[live]
    keys: ShellMenu,
    #[live]
    osd: ShellOsd,
    #[live]
    notes: ShellNotifications,
    #[live]
    panel: ShellPanel,
    /// A second flyout instance: the one the BAR opens, floating over the
    /// whole gallery like it does over the desktop.
    #[live]
    bar_panel: ShellPanel,
    #[rust]
    area: Area,
    #[rust]
    started: bool,
    #[rust]
    scroll: f64,
    #[rust]
    demo_volume: u32,
    #[rust]
    demo_toggle: bool,
}

impl ShellGallery {
    fn start(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        self.demo_volume = 45;
        self.demo_toggle = true;
        self.bar.data = BarData::fixture();
        self.bar.pad_left = 8.0;
        self.menu.open_at(cx, "", MenuSkin::Menu);
        self.launcher.open_at(cx, "apps", MenuSkin::Launcher);
        self.keys.open_at(cx, "learn.keybindings", MenuSkin::Menu);
        self.panel.data = PanelData::fixture();
        self.panel.open = Some(PanelKind::Clock);
        self.bar_panel.data = PanelData::fixture();
        self.osd.show = Some(OsdShow::volume(62, false));
        // The gallery's OSD never times out — it is a picture of one.
        if let Some(show) = self.osd.show.as_mut() {
            show.duration = 0.0;
        }
        for note in fixtures() {
            self.notes.post(cx, note);
        }
        self.notes.bar_clearance = 0.0;
    }

    /// The demo box every section draws its component in.
    fn demo_box(&mut self, cx: &mut Cx2d, r: Rect) {
        let fg = self.tokens.popups.text;
        self.d
            .bordered(cx, r, alpha(fg, 0.04), alpha(fg, 0.10), alpha(fg, 0.10), 0.0, 1.0);
    }

    fn draw_typography(&mut self, cx: &mut Cx2d, r: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let rows: [(&str, f64); 9] = [
            ("caption", tok.font.caption),
            ("bodySmall", tok.font.body_small),
            ("body", tok.font.body),
            ("subtitle", tok.font.subtitle),
            ("title", tok.font.title),
            ("heading", tok.font.heading),
            ("display", tok.font.display),
            ("displayLarge", tok.font.display_large),
            ("iconLarge", tok.font.icon_large),
        ];
        let mut y = r.pos.y + 6.0;
        for (name, px) in rows {
            let h = (px * 1.5).max(16.0);
            let row = rect(r.pos.x + 10.0, y, r.size.x - 20.0, h);
            self.d.label(
                cx,
                rect(row.pos.x, row.pos.y, 110.0, h),
                false,
                tok.font.body_small,
                super::darker(fg, 1.4),
                super::ui::HAlign::Left,
                name,
            );
            self.d.label(
                cx,
                rect(row.pos.x + 110.0, row.pos.y, 50.0, h),
                false,
                tok.font.body_small,
                super::darker(fg, 1.6),
                super::ui::HAlign::Left,
                &format!("{} px", px),
            );
            self.d.label(
                cx,
                rect(row.pos.x + 170.0, row.pos.y, row.size.x - 170.0, h),
                false,
                px,
                fg,
                super::ui::HAlign::Left,
                "The quick brown fox",
            );
            y += h;
        }
    }

    fn draw_controls(&mut self, cx: &mut Cx2d, r: Rect) {
        let tok = self.tokens;
        let fg = tok.popups.text;
        let pad = 12.0;
        let (row1, rest) = cut_top(inset(r, pad), tok.spacing.control_height);
        // Every button state side by side, `controlGap` apart.
        let states = [
            ("Idle", CtrlState::Normal),
            ("Hover", CtrlState::Hover),
            ("Selected", CtrlState::Selected),
            ("Pressed", CtrlState::Pressed),
            ("Disabled", CtrlState::Disabled),
        ];
        let mut x = row1.pos.x;
        for (label, state) in states {
            let w = self.d.button_width(cx, &tok, false, label, tok.font.body);
            let cell = rect(x, row1.pos.y, w, row1.size.y);
            self.d
                .button(cx, cell, &tok, state, None, label, tok.font.body, fg, true);
            x += w + tok.spacing.control_gap;
        }
        // An icon button too.
        let icon_w = tok.spacing.control_height;
        self.d.button(
            cx,
            rect(x, row1.pos.y, icon_w, row1.size.y),
            &tok,
            CtrlState::Normal,
            Some(Ico::Check),
            "",
            tok.font.body,
            fg,
            true,
        );

        let (row2, rest) = cut_top(rest, tok.spacing.panel_gap + 30.0);
        let switch = self.d.toggle_switch(
            cx,
            dvec2(row2.pos.x, row2.pos.y + tok.spacing.panel_gap),
            &tok,
            self.demo_toggle,
            fg,
        );
        let slider = rect(
            switch.pos.x + switch.size.x + tok.spacing.xl,
            row2.pos.y + tok.spacing.panel_gap - 4.0,
            220.0,
            switch.size.y + 8.0,
        );
        self.d.panel_slider(
            cx,
            slider,
            &tok,
            self.demo_volume as f64 / 100.0,
            fg,
            tok.popups.background,
            false,
        );
        self.d.label(
            cx,
            rect(slider.pos.x + slider.size.x + 8.0, slider.pos.y, 44.0, slider.size.y),
            true,
            tok.font.caption,
            fg,
            super::ui::HAlign::Right,
            &format!("{}%", self.demo_volume),
        );
        let field = rect(
            slider.pos.x + slider.size.x + 60.0,
            slider.pos.y,
            180.0,
            tok.spacing.control_height,
        );
        self.d.text_field(
            cx,
            field,
            &tok,
            "",
            "Type something",
            false,
            false,
            fg,
        );

        // Section header, separator, then three cursor-surface rows.
        let (head, rest) = cut_top(rest, 20.0);
        self.d.section_header(cx, head, &tok, fg, "CURSOR SURFACE");
        let (sep, mut rest) = cut_top(rest, 8.0);
        self.d.separator(cx, sep, fg, 0.12);
        for (i, label) in ["Idle row", "Row under the cursor", "Current row"]
            .iter()
            .enumerate()
        {
            let (row, next) = cut_top(rest, tok.spacing.popup_row_height);
            rest = next;
            self.d
                .cursor_surface(cx, row, &tok.controls, i == 1, i == 2);
            self.d.label(
                cx,
                inset(row, 8.0),
                i == 2,
                tok.font.body,
                fg,
                super::ui::HAlign::Left,
                label,
            );
        }
    }

    fn draw_panels_row(&mut self, cx: &mut Cx2d, r: Rect) {
        // Two rows of two, each panel anchored to a fake bar module above
        // it so it lands where it would under the real bar.
        let rows: [[PanelKind; 2]; 2] = [
            [PanelKind::Clock, PanelKind::Audio],
            [PanelKind::Power, PanelKind::Monitor],
        ];
        let row_h = (r.size.y - 8.0) * 0.5;
        let mut clock_slot = r;
        for (ri, kinds) in rows.iter().enumerate() {
            let mut x = r.pos.x + 8.0;
            let y = r.pos.y + ri as f64 * (row_h + 8.0);
            for kind in kinds {
                let w = kind.content_width() + self.tokens.spacing.popup_padding * 2.0 + 20.0;
                let slot = rect(x, y, w, row_h);
                if *kind == PanelKind::Clock {
                    clock_slot = slot;
                }
                self.panel.open = Some(*kind);
                self.panel.anchor = rect(slot.pos.x + w * 0.5 - 13.0, slot.pos.y, 27.0, 0.0);
                self.panel.draw_surface(cx, slot);
                x += w + 8.0;
            }
        }
        // The clock is the live one: draw it last so its hit rects win.
        self.panel.open = Some(PanelKind::Clock);
        self.panel.anchor = rect(
            clock_slot.pos.x + clock_slot.size.x * 0.5 - 13.0,
            clock_slot.pos.y,
            27.0,
            0.0,
        );
        self.panel.draw_surface(cx, clock_slot);
    }
}

impl Widget for ShellGallery {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.started {
            self.start(cx);
        }
        cx.begin_turtle(walk, self.layout);
        let r = cx.turtle().rect();
        let tok = self.tokens;
        let fg = tok.popups.text;
        self.draw_bg.color = alpha(tok.popups.background, 1.0);
        self.draw_bg.draw_abs(cx, r);

        let pad = tok.spacing.panel_padding;
        let mut y = r.pos.y + pad - self.scroll;
        let width = r.size.x - pad * 2.0;

        // Title.
        let title = rect(r.pos.x + pad, y, width, 30.0);
        self.d.label(
            cx,
            title,
            true,
            tok.font.icon_large,
            fg,
            super::ui::HAlign::Left,
            "mpwm shell — omarchy surfaces",
        );
        y += 30.0;
        let sub = rect(r.pos.x + pad, y, width, 20.0);
        self.d.label(
            cx,
            sub,
            false,
            tok.font.body_small,
            super::darker(fg, 1.4),
            super::ui::HAlign::Left,
            "Every surface with fixture data, drawn from mod.mpwm_theme.shell.",
        );
        y += 26.0;
        self.d
            .separator(cx, rect(r.pos.x + pad, y, width, 1.0), fg, 0.12);
        y += 16.0;

        for section in SECTIONS {
            let head = rect(r.pos.x + pad, y, width, 20.0);
            self.d.label(
                cx,
                head,
                true,
                tok.font.subtitle,
                fg,
                super::ui::HAlign::Left,
                section.title,
            );
            y += 20.0;
            let desc_lines = self
                .d
                .wrap(cx, false, tok.font.caption, section.body, width, 3);
            for line in &desc_lines {
                self.d.label(
                    cx,
                    rect(r.pos.x + pad, y, width, 14.0),
                    false,
                    tok.font.caption,
                    super::darker(fg, 1.5),
                    super::ui::HAlign::Left,
                    line,
                );
                y += 14.0;
            }
            y += 6.0;
            let demo = rect(r.pos.x + pad, y, width, section.height);
            match section.title {
                "Bar" => {
                    self.bar.draw_bar(cx, demo);
                }
                "Typography" => {
                    self.demo_box(cx, demo);
                    self.draw_typography(cx, demo);
                }
                "Controls" => {
                    self.demo_box(cx, demo);
                    self.draw_controls(cx, demo);
                }
                "Menu" => {
                    self.demo_box(cx, demo);
                    self.menu.draw_surface(cx, demo);
                }
                "Launcher" => {
                    self.demo_box(cx, demo);
                    self.launcher.draw_surface(cx, demo);
                }
                "Keybindings" => {
                    self.demo_box(cx, demo);
                    self.keys.draw_surface(cx, demo);
                }
                "Panels" => {
                    self.demo_box(cx, demo);
                    self.draw_panels_row(cx, demo);
                }
                "OSD" => {
                    self.demo_box(cx, demo);
                    self.osd.draw_surface(cx, demo);
                }
                "Notifications" => {
                    self.demo_box(cx, demo);
                    self.notes.draw_surface(cx, demo);
                }
                _ => {}
            }
            y += section.height + 24.0;
        }

        // The bar's own flyout floats over everything, last.
        if self.bar_panel.open.is_some() {
            self.bar_panel.draw_surface(cx, r);
        }

        let _ = scope;
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Never self-start from EVENTS: an invisible widget still receives
        // them, so the hidden desktop-mode gallery was opening its fixture
        // menus and answering stray keys with real Activate actions — the
        // ghost app launches and the menu-card flash. Fixtures start on
        // the first DRAW (which only happens when the gallery is shown).
        if !self.started {
            return;
        }
        // The toasts really do expire (8s Normal, 5s Low, never for
        // Critical); the gallery re-posts the set once any of them goes, so
        // the stack and its countdown are always something you can watch.
        if self.notes.len() < fixtures().len() {
            self.notes.clear(cx);
            for note in fixtures() {
                self.notes.post(cx, note);
            }
        }
        // The three menu surfaces, the panel and the toasts are all live.
        self.menu.handle_event(cx, event, scope);
        self.launcher.handle_event(cx, event, scope);
        self.keys.handle_event(cx, event, scope);
        self.panel.handle_event(cx, event, scope);
        self.bar_panel.handle_event(cx, event, scope);
        self.notes.handle_event(cx, event, scope);
        self.bar.handle_event(cx, event, scope);
        self.osd.handle_event(cx, event, scope);

        if let Event::Scroll(e) = event {
            let r = self.area.rect(cx);
            if super::ui::contains(r, e.abs) {
                self.scroll = (self.scroll + e.scroll.y).max(0.0);
                self.redraw(cx);
            }
        }
        if let Event::KeyDown(e) = event {
            // The menu owns the keyboard while it is open, like the real one.
            let screen = self.area.rect(cx);
            if self.menu.handle_key(cx, e, screen) {
                self.redraw(cx);
            }
        }
    }
}

impl ShellGallery {
    /// Route the actions the surfaces raise — the bar opens panels, the
    /// menu prints what it would run, the panels move the demo values.
    pub fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            match wa.cast::<ShellBarAction>() {
                ShellBarAction::Press(module) => {
                    if let Some(kind) = PanelKind::for_module(module) {
                        let anchor = self.bar.module_rect(module).unwrap_or_default();
                        self.bar_panel.toggle(cx, kind, anchor);
                        self.bar.data.open_panel = self.bar_panel.open.map(|k| k.module());
                    }
                    if module == BarModule::Menu {
                        self.menu.open_at(cx, "", MenuSkin::Menu);
                    }
                    self.redraw(cx);
                }
                ShellBarAction::Wheel(module, dir) => {
                    if module == BarModule::Audio {
                        let level = self.bar.data.volume.unwrap_or(0) as f64;
                        let next = (level + dir * 5.0).clamp(0.0, 100.0) as u32;
                        self.bar.data.volume = Some(next);
                        self.osd.present(cx, OsdShow::volume(next, false));
                        self.redraw(cx);
                    }
                }
                _ => {}
            }
            match wa.cast::<ShellPanelAction>() {
                ShellPanelAction::SetVolume(v) => {
                    self.bar_panel.data.volume = Some(v);
                    self.panel.data.volume = Some(v);
                    self.bar.data.volume = Some(v);
                    self.demo_volume = v;
                    self.osd.present(cx, OsdShow::volume(v, self.panel.data.muted));
                    self.redraw(cx);
                }
                ShellPanelAction::ToggleMute => {
                    self.bar_panel.data.muted = !self.panel.data.muted;
                    self.panel.data.muted = !self.panel.data.muted;
                    self.bar.data.muted = self.panel.data.muted;
                    self.demo_toggle = !self.panel.data.muted;
                    self.redraw(cx);
                }
                ShellPanelAction::SetBrightness(v) => {
                    self.bar_panel.data.brightness = Some(v);
                    self.panel.data.brightness = Some(v);
                    self.osd.present(cx, OsdShow::brightness(v));
                    self.redraw(cx);
                }
                ShellPanelAction::SetTextSize(px) => {
                    self.panel.data.text_size = px;
                    self.redraw(cx);
                }
                ShellPanelAction::Close => {
                    self.bar.data.open_panel = None;
                    self.redraw(cx);
                }
                ShellPanelAction::None => {}
            }
            match wa.cast::<ShellMenuAction>() {
                ShellMenuAction::Activate(target) => {
                    log!("gallery: menu activated '{}'", target);
                    self.menu.open_at(cx, "", MenuSkin::Menu);
                    self.redraw(cx);
                }
                ShellMenuAction::Cancel => {
                    // The gallery keeps its menus up: reopen where we were.
                    self.menu.open_at(cx, "", MenuSkin::Menu);
                    self.redraw(cx);
                }
                ShellMenuAction::None => {}
            }
        }
    }

    /// `--gallery` present in the command line.
    pub fn requested() -> bool {
        std::env::args().any(|a| a == "--gallery")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_surface_has_a_section() {
        let titles: Vec<&str> = SECTIONS.iter().map(|s| s.title).collect();
        for want in [
            "Bar",
            "Typography",
            "Controls",
            "Menu",
            "Launcher",
            "Keybindings",
            "Panels",
            "OSD",
            "Notifications",
        ] {
            assert!(titles.contains(&want), "missing gallery section {}", want);
        }
    }
}
