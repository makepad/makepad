//! Sheets reads its colours from the active system theme.

use makepad_widgets::*;

/// The few colours the Rust side paints directly (the fill handle, error text).
pub struct Colors {
    pub accent: Vec4f,
    pub red: Vec4f,
    pub green: Vec4f,
    pub bg: Vec4f,
    pub bg_light: Vec4f,
    pub fg: Vec4f,
    pub fg_dark: Vec4f,
}

pub fn colors(cx: &mut Cx) -> Colors {
    cx.with_vm(|vm| {
        let mut color = |role| theme_tokens::color(vm, role)
            .unwrap_or_else(|| panic!("missing system theme colour: {role}"));
        Colors {
            accent: color("color_focus"),
            red: color("color_error"),
            green: color("color_success"),
            bg: color("color_bg_app"),
            bg_light: color("color_fg_app"),
            fg: color("color_text"),
            fg_dark: color("color_text_disabled"),
        }
    })
}

/// Bind the sheet's drawing roles after `makepad_widgets::script_mod` and
/// before this crate's own modules. Re-evaluate on every style reload.
pub fn install(vm: &mut ScriptVm) {
    vm.eval(script! {
        use mod.prelude.widgets.*
        mod.sheets = {
            bg: theme.color_bg_app
            bg_dark: theme.color_bg_container
            bg_light: theme.color_fg_app
            fg: theme.color_text
            fg_bright: theme.color_text_hover
            fg_dark: theme.color_text_disabled
            accent: theme.color_focus
            selection: theme.color_bg_highlight
            muted: theme.color_bevel_outset_2
            red: theme.color_error
            green: theme.color_success
            yellow: theme.color_warning
            blue: theme.color_info
            sel_fill: theme.color_bg_highlight * vec4(1.0, 1.0, 1.0, 0.4)
            accent_ghost: theme.color_focus * vec4(1.0, 1.0, 1.0, 0.26666667)
        }
        true
    });
    for e in vm.take_errors() {
        log!("sheets theme: {}", e);
    }
}
