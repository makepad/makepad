//! The omarchy-shell desktop UI, ported from the Quickshell/QML original
//! (`local/vendor/omarchy/shell/`) to splash/makepad.
//!
//! Everything here is read from that source: the spacing scale and type
//! scale of `Commons/Style.qml`, the surface roles of `Commons/Color.qml`,
//! the per-surface token contract of `default/themed/shell.toml.tpl`, the
//! bar layout of `config/omarchy/shell.json`, and each plugin's own QML.
//!
//! # The token contract
//!
//! At runtime the ONLY source is the splash theme: `mod.wm_theme.shell`,
//! an object with one section per surface, mirroring `shell.toml.tpl`
//! key-for-key (dashes become underscores, `[section] key` becomes
//! `section: {key: ...}`). `theme.rs` fills it in from the theme's palette
//! the way `omarchy-theme-set-templates` fills the `.tpl`, and a theme may
//! ship its own `shell: { ... }` block to override the lot.
//!
//! Sizes are in PIXELS, like the QML. Makepad's `font_size` is in points,
//! so text is drawn at `px * 0.75` (see `ShellDraw::px_to_pt`).

use makepad_widgets::*;

pub mod bar;
pub mod gallery;
pub mod launcher;
pub mod menu;
pub mod notifications;
pub mod osd;
pub mod panels;
pub mod ui;

/// Register every shell widget. Called from `AppMain::script_mod` AFTER
/// the theme has been evaluated (the DSL below reads `mod.wm_theme`).
pub fn script_mod(vm: &mut ScriptVm) {
    ui::script_mod(vm);
    bar::script_mod(vm);
    menu::script_mod(vm);
    osd::script_mod(vm);
    notifications::script_mod(vm);
    panels::script_mod(vm);
    gallery::script_mod(vm);
}

// ======================================================================
// Tokens — `Commons/Style.qml` + `default/themed/shell.toml.tpl`
// ======================================================================

/// `[bar]` plus `Style.bar.*`.
#[derive(Clone, Copy, Debug, Script, ScriptHook)]
pub struct BarTokens {
    #[live]
    pub background: Vec4f,
    #[live(1.0)]
    pub background_alpha: f32,
    #[live]
    pub text: Vec4f,
    /// Modules calling attention to themselves — `{{ red }}`.
    #[live]
    pub active: Vec4f,
    #[live(26.0)]
    pub size_horizontal: f64,
    #[live(28.0)]
    pub size_vertical: f64,
    #[live(27.0)]
    pub icon_slot: f64,
    #[live(16.0)]
    pub icon_canvas: f64,
    #[live(13.0)]
    pub icon_font: f64,
    #[live(21.0)]
    pub status_slot: f64,
}

impl Default for BarTokens {
    fn default() -> Self {
        Self {
            background: rgb(0x10, 0x13, 0x15),
            background_alpha: 1.0,
            text: rgb(0xca, 0xcc, 0xcc),
            active: rgb(0xa5, 0x55, 0x55),
            size_horizontal: 26.0,
            size_vertical: 28.0,
            icon_slot: 27.0,
            icon_canvas: 16.0,
            icon_font: 13.0,
            status_slot: 21.0,
        }
    }
}

/// A card surface: `[popups]`, `[tooltip]`. `border` may be a hyprland
/// gradient, so it carries a second stop and an angle (`shell_gradient`).
#[derive(Clone, Copy, Debug, Script, ScriptHook)]
pub struct SurfaceTokens {
    #[live]
    pub background: Vec4f,
    #[live(1.0)]
    pub background_alpha: f32,
    #[live]
    pub text: Vec4f,
    #[live]
    pub border: Vec4f,
    #[live]
    pub border_end: Vec4f,
    #[live(0.0)]
    pub border_angle: f32,
    #[live(1.0)]
    pub border_alpha: f32,
    #[live(2.0)]
    pub border_width: f64,
}

impl Default for SurfaceTokens {
    fn default() -> Self {
        Self {
            background: rgb(0x10, 0x13, 0x15),
            background_alpha: 1.0,
            text: rgb(0xca, 0xcc, 0xcc),
            border: rgb(0xca, 0xcc, 0xcc),
            border_end: rgb(0xca, 0xcc, 0xcc),
            border_angle: 0.0,
            border_alpha: 1.0,
            border_width: 2.0,
        }
    }
}

/// `[notifications]` — a card plus the countdown color.
#[derive(Clone, Copy, Debug, Script, ScriptHook)]
pub struct NotificationTokens {
    #[live]
    pub surface: SurfaceTokens,
    #[live]
    pub countdown: Vec4f,
}

impl Default for NotificationTokens {
    fn default() -> Self {
        Self {
            surface: SurfaceTokens::default(),
            countdown: rgb(0xca, 0xcc, 0xcc),
        }
    }
}

/// `[menu]` / `[launcher]` — the same six-token contract.
#[derive(Clone, Copy, Debug, Script, ScriptHook)]
pub struct MenuTokens {
    #[live]
    pub surface: SurfaceTokens,
    #[live]
    pub scrim: Vec4f,
    #[live(0.5)]
    pub scrim_alpha: f32,
    #[live]
    pub selected_background: Vec4f,
    #[live(0.08)]
    pub selected_background_alpha: f32,
    #[live]
    pub selected_text: Vec4f,
    #[live]
    pub selected_border: Vec4f,
    #[live(0.25)]
    pub selected_border_alpha: f32,
}

impl Default for MenuTokens {
    fn default() -> Self {
        Self {
            surface: SurfaceTokens::default(),
            scrim: rgb(0x10, 0x13, 0x15),
            scrim_alpha: 0.5,
            selected_background: rgb(0xca, 0xcc, 0xcc),
            selected_background_alpha: 0.08,
            selected_text: rgb(0xca, 0xcc, 0xcc),
            selected_border: rgb(0xca, 0xcc, 0xcc),
            selected_border_alpha: 0.25,
        }
    }
}

/// `[controls]` — the shared interactive-state vocabulary every control
/// in `Ui/` paints itself with.
#[derive(Clone, Copy, Debug, Script, ScriptHook)]
pub struct ControlTokens {
    #[live]
    pub normal_color: Vec4f,
    #[live(0.04)]
    pub normal_fill_alpha: f32,
    #[live]
    pub normal_border: Vec4f,
    #[live(1.0)]
    pub normal_border_width: f64,
    #[live(0.4)]
    pub normal_border_alpha: f32,

    #[live]
    pub hover_color: Vec4f,
    #[live(0.08)]
    pub hover_fill_alpha: f32,
    #[live]
    pub hover_border: Vec4f,
    #[live(1.0)]
    pub hover_border_width: f64,
    #[live(0.25)]
    pub hover_border_alpha: f32,

    #[live]
    pub focus_color: Vec4f,
    #[live(0.08)]
    pub focus_fill_alpha: f32,
    #[live]
    pub focus_border: Vec4f,
    #[live(1.0)]
    pub focus_border_width: f64,
    #[live(0.25)]
    pub focus_border_alpha: f32,

    #[live]
    pub selected_color: Vec4f,
    #[live(0.18)]
    pub selected_fill_alpha: f32,
    #[live]
    pub selected_border: Vec4f,
    #[live(0.0)]
    pub selected_border_width: f64,
    #[live(1.0)]
    pub selected_border_alpha: f32,

    #[live(0.22)]
    pub pressed_fill_alpha: f32,
    #[live(0.35)]
    pub selection_fill_alpha: f32,
}

impl Default for ControlTokens {
    fn default() -> Self {
        let fg = rgb(0xca, 0xcc, 0xcc);
        Self {
            normal_color: fg,
            normal_fill_alpha: 0.04,
            normal_border: fg,
            normal_border_width: 1.0,
            normal_border_alpha: 0.4,
            hover_color: fg,
            hover_fill_alpha: 0.08,
            hover_border: fg,
            hover_border_width: 1.0,
            hover_border_alpha: 0.25,
            focus_color: fg,
            focus_fill_alpha: 0.08,
            focus_border: fg,
            focus_border_width: 1.0,
            focus_border_alpha: 0.25,
            selected_color: fg,
            selected_fill_alpha: 0.18,
            selected_border: fg,
            selected_border_width: 0.0,
            selected_border_alpha: 1.0,
            pressed_fill_alpha: 0.22,
            selection_fill_alpha: 0.35,
        }
    }
}

/// `[spacing]` — `Style.spacing.*`, in px at scale 1.0.
#[derive(Clone, Copy, Debug, Script, ScriptHook)]
pub struct SpacingTokens {
    #[live(2.0)]
    pub xxs: f64,
    #[live(3.0)]
    pub xs: f64,
    #[live(4.0)]
    pub sm: f64,
    #[live(6.0)]
    pub md: f64,
    #[live(8.0)]
    pub lg: f64,
    #[live(10.0)]
    pub xl: f64,
    #[live(12.0)]
    pub xxl: f64,
    #[live(14.0)]
    pub xxxl: f64,
    #[live(18.0)]
    pub huge: f64,
    #[live(8.0)]
    pub control_gap: f64,
    #[live(10.0)]
    pub control_padding_x: f64,
    #[live(6.0)]
    pub control_padding_y: f64,
    #[live(7.0)]
    pub input_padding_y: f64,
    #[live(28.0)]
    pub control_height: f64,
    #[live(28.0)]
    pub popup_row_height: f64,
    #[live(240.0)]
    pub dropdown_width: f64,
    #[live(260.0)]
    pub searchable_dropdown_width: f64,
    #[live(120.0)]
    pub number_field_width: f64,
    #[live(220.0)]
    pub searchable_popup_min_height: f64,
    #[live(8.0)]
    pub row_gap: f64,
    #[live(12.0)]
    pub row_padding_x: f64,
    #[live(4.0)]
    pub label_gap: f64,
    #[live(14.0)]
    pub panel_gap: f64,
    #[live(18.0)]
    pub panel_padding: f64,
    #[live(14.0)]
    pub popup_padding: f64,
    /// `Style.gapsOut` — half of hyprland's `general:gaps_out`.
    #[live(5.0)]
    pub gaps_out: f64,
}

impl Default for SpacingTokens {
    fn default() -> Self {
        Self {
            xxs: 2.0,
            xs: 3.0,
            sm: 4.0,
            md: 6.0,
            lg: 8.0,
            xl: 10.0,
            xxl: 12.0,
            xxxl: 14.0,
            huge: 18.0,
            control_gap: 8.0,
            control_padding_x: 10.0,
            control_padding_y: 6.0,
            input_padding_y: 7.0,
            control_height: 28.0,
            popup_row_height: 28.0,
            dropdown_width: 240.0,
            searchable_dropdown_width: 260.0,
            number_field_width: 120.0,
            searchable_popup_min_height: 220.0,
            row_gap: 8.0,
            row_padding_x: 12.0,
            label_gap: 4.0,
            panel_gap: 14.0,
            panel_padding: 18.0,
            popup_padding: 14.0,
            gaps_out: 5.0,
        }
    }
}

/// `[font]` — `Style.font.*`, in px (base 12 with the .tpl multipliers).
#[derive(Clone, Copy, Debug, Script, ScriptHook)]
pub struct FontTokens {
    #[live(12.0)]
    pub base_size: f64,
    #[live(10.0)]
    pub caption: f64,
    #[live(11.0)]
    pub body_small: f64,
    #[live(12.0)]
    pub body: f64,
    #[live(13.0)]
    pub subtitle: f64,
    #[live(14.0)]
    pub title: f64,
    #[live(16.0)]
    pub heading: f64,
    #[live(24.0)]
    pub display: f64,
    #[live(28.0)]
    pub display_large: f64,
    #[live(11.0)]
    pub icon_small: f64,
    #[live(14.0)]
    pub icon: f64,
    #[live(18.0)]
    pub icon_large: f64,
}

impl Default for FontTokens {
    fn default() -> Self {
        Self {
            base_size: 12.0,
            caption: 10.0,
            body_small: 11.0,
            body: 12.0,
            subtitle: 13.0,
            title: 14.0,
            heading: 16.0,
            display: 24.0,
            display_large: 28.0,
            icon_small: 11.0,
            icon: 14.0,
            icon_large: 18.0,
        }
    }
}

/// The whole `mod.wm_theme.shell` object.
#[derive(Clone, Copy, Debug, Script, ScriptHook)]
pub struct ShellTokens {
    #[live]
    pub bar: BarTokens,
    #[live]
    pub popups: SurfaceTokens,
    #[live]
    pub tooltip: SurfaceTokens,
    #[live]
    pub notifications: NotificationTokens,
    #[live]
    pub menu: MenuTokens,
    #[live]
    pub launcher: MenuTokens,
    #[live]
    pub controls: ControlTokens,
    #[live]
    pub spacing: SpacingTokens,
    #[live]
    pub font: FontTokens,
    /// `cornerRadius` — 0 in omarchy, and every surface here draws hard
    /// square corners. Carried so a theme CAN say otherwise one day.
    #[live(0.0)]
    pub corner_radius: f64,
}

impl Default for ShellTokens {
    fn default() -> Self {
        let mut launcher = MenuTokens::default();
        launcher.surface.background_alpha = 0.95;
        Self {
            bar: BarTokens::default(),
            popups: SurfaceTokens::default(),
            tooltip: SurfaceTokens {
                background_alpha: 0.97,
                border_width: 1.0,
                ..SurfaceTokens::default()
            },
            notifications: NotificationTokens::default(),
            menu: MenuTokens::default(),
            launcher,
            controls: ControlTokens::default(),
            spacing: SpacingTokens::default(),
            font: FontTokens::default(),
            corner_radius: 0.0,
        }
    }
}

// ----------------------------------------------------------------------
// Color helpers (`Commons/Util.qml` alpha + `Color.composed`)
// ----------------------------------------------------------------------

pub fn rgb(r: u8, g: u8, b: u8) -> Vec4f {
    Vec4f {
        x: r as f32 / 255.0,
        y: g as f32 / 255.0,
        z: b as f32 / 255.0,
        w: 1.0,
    }
}

/// `Util.alpha(color, a)` — replace the alpha, keep the color.
pub fn alpha(c: Vec4f, a: f32) -> Vec4f {
    Vec4f {
        w: a.clamp(0.0, 1.0),
        ..c
    }
}

/// Multiply into the existing alpha (fading a whole surface out).
pub fn fade(c: Vec4f, a: f32) -> Vec4f {
    Vec4f { w: c.w * a, ..c }
}

/// `Qt.darker(color, f)` — the panels dim their secondary text with it.
pub fn darker(c: Vec4f, f: f32) -> Vec4f {
    Vec4f {
        x: c.x / f,
        y: c.y / f,
        z: c.z / f,
        w: c.w,
    }
}

impl SurfaceTokens {
    /// The card fill, alpha companion applied.
    pub fn bg(&self) -> Vec4f {
        alpha(self.background, self.background_alpha)
    }
    pub fn border_start(&self) -> Vec4f {
        alpha(self.border, self.border_alpha)
    }
    pub fn border_stop(&self) -> Vec4f {
        alpha(self.border_end, self.border_alpha)
    }
}

impl MenuTokens {
    pub fn scrim_color(&self) -> Vec4f {
        alpha(self.scrim, self.scrim_alpha)
    }
    pub fn selected_bg(&self) -> Vec4f {
        alpha(self.selected_background, self.selected_background_alpha)
    }
    pub fn selected_border_color(&self) -> Vec4f {
        alpha(self.selected_border, self.selected_border_alpha)
    }
}

/// The state a control paints itself in — `Style.controlFill` /
/// `controlBorder` / `controlBorderWidth`'s focus > hover > normal chain,
/// with `selected` and `pressed` on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CtrlState {
    #[default]
    Normal,
    /// Mouse hover OR the panel keyboard cursor (`hasCursor`).
    Hover,
    Focus,
    /// The persistent chosen/current state.
    Selected,
    Pressed,
    /// Listed but not actionable: dimmed, no chrome.
    Disabled,
}

impl ControlTokens {
    pub fn fill(&self, state: CtrlState) -> Vec4f {
        match state {
            CtrlState::Normal => alpha(self.normal_color, self.normal_fill_alpha),
            CtrlState::Hover => alpha(self.hover_color, self.hover_fill_alpha),
            CtrlState::Focus => alpha(self.focus_color, self.focus_fill_alpha),
            CtrlState::Selected => alpha(self.selected_color, self.selected_fill_alpha),
            CtrlState::Pressed => alpha(self.hover_color, self.pressed_fill_alpha),
            CtrlState::Disabled => alpha(self.normal_color, self.normal_fill_alpha * 0.5),
        }
    }

    pub fn border(&self, state: CtrlState) -> Vec4f {
        match state {
            CtrlState::Normal => alpha(self.normal_border, self.normal_border_alpha),
            CtrlState::Hover | CtrlState::Pressed => {
                alpha(self.hover_border, self.hover_border_alpha)
            }
            CtrlState::Focus => alpha(self.focus_border, self.focus_border_alpha),
            CtrlState::Selected => alpha(self.selected_border, self.selected_border_alpha),
            CtrlState::Disabled => alpha(self.normal_border, self.normal_border_alpha * 0.5),
        }
    }

    pub fn border_width(&self, state: CtrlState) -> f64 {
        match state {
            CtrlState::Normal | CtrlState::Disabled => self.normal_border_width,
            CtrlState::Hover | CtrlState::Pressed => self.hover_border_width,
            CtrlState::Focus => self.focus_border_width,
            CtrlState::Selected => self.selected_border_width,
        }
    }

    /// `CursorSurface`: transparent unless the cursor is on it or it is
    /// the current row — the panels' row chrome.
    pub fn cursor_fill(&self, has_cursor: bool, current: bool) -> Vec4f {
        if current {
            self.fill(CtrlState::Selected)
        } else if has_cursor {
            self.fill(CtrlState::Hover)
        } else {
            Vec4f {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 0.0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_omarchy_scale() {
        let t = ShellTokens::default();
        // Style.qml spacing scale.
        assert_eq!(t.spacing.control_height, 28.0);
        assert_eq!(t.spacing.popup_row_height, 28.0);
        assert_eq!(t.spacing.panel_padding, 18.0);
        assert_eq!(t.spacing.popup_padding, 14.0);
        assert_eq!(t.spacing.dropdown_width, 240.0);
        // Type scale, base 12.
        assert_eq!(t.font.body, 12.0);
        assert_eq!(t.font.title, 14.0);
        assert_eq!(t.font.display_large, 28.0);
        // Bar.
        assert_eq!(t.bar.size_horizontal, 26.0);
        assert_eq!(t.bar.status_slot, 21.0);
        // Hard corners.
        assert_eq!(t.corner_radius, 0.0);
        // The launcher card is the translucent one.
        assert_eq!(t.launcher.surface.background_alpha, 0.95);
        assert_eq!(t.menu.surface.background_alpha, 1.0);
        assert_eq!(t.tooltip.background_alpha, 0.97);
    }

    #[test]
    fn control_states_use_the_documented_alphas() {
        let c = ControlTokens::default();
        assert_eq!(c.fill(CtrlState::Normal).w, 0.04);
        assert_eq!(c.fill(CtrlState::Hover).w, 0.08);
        assert_eq!(c.fill(CtrlState::Selected).w, 0.18);
        assert_eq!(c.fill(CtrlState::Pressed).w, 0.22);
        assert_eq!(c.border(CtrlState::Normal).w, 0.4);
        assert_eq!(c.border(CtrlState::Hover).w, 0.25);
        assert_eq!(c.border_width(CtrlState::Normal), 1.0);
        // Selected drops its border (selected-border-width = 0).
        assert_eq!(c.border_width(CtrlState::Selected), 0.0);
    }
}
