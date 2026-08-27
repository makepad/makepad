//! The palette bridge. `mp_theme::apply` retints the *stock* widgets from the
//! window manager's theme.splash; this module carries the same palette into
//! `mod.sheets`, which is where mpsheets' own splash reads its colours from.
//!
//! Standalone (no `MPWM_THEME_SPLASH`) every key falls back to Tokyo Night, so
//! the app is dark whether or not the WM is running.

use makepad_widgets::*;

/// key, fallback — the whole surface mpsheets paints with.
const KEYS: &[(&str, &str)] = &[
    ("background", "#1a1b26"),
    ("darker_background", "#0e0e14"),
    ("lighter_background", "#24283b"),
    ("foreground", "#a9b1d6"),
    ("bright_foreground", "#c0caf5"),
    ("dark_foreground", "#565f89"),
    ("accent", "#7aa2f7"),
    ("selection", "#292e42"),
    ("muted", "#414868"),
    ("red", "#f7768e"),
    ("green", "#9ece6a"),
    ("yellow", "#e0af68"),
    ("blue", "#7aa2f7"),
];

fn hex_of(key: &str) -> String {
    let fallback = KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, f)| *f)
        .unwrap_or("#ff00ff");
    match mp_theme::current() {
        Some(p) => p.hex(key, fallback),
        None => fallback.to_string(),
    }
}

/// Parse `#rrggbb` / `#rrggbbaa` the way the splash tokenizer does.
pub fn rgba(hex: &str) -> Vec4f {
    let s = hex.trim().trim_start_matches('#');
    let bytes = s.as_bytes();
    let nib = |i: usize| -> f32 {
        let c = bytes.get(i).copied().unwrap_or(b'0');
        let v = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => 0,
        };
        v as f32
    };
    let byte = |i: usize| (nib(i * 2) * 16.0 + nib(i * 2 + 1)) / 255.0;
    match bytes.len() {
        3 => vec4(
            nib(0) / 15.0,
            nib(1) / 15.0,
            nib(2) / 15.0,
            1.0,
        ),
        8 => vec4(byte(0), byte(1), byte(2), byte(3)),
        _ => vec4(byte(0), byte(1), byte(2), 1.0),
    }
}

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

pub fn colors() -> Colors {
    Colors {
        accent: rgba(&hex_of("accent")),
        red: rgba(&hex_of("red")),
        green: rgba(&hex_of("green")),
        bg: rgba(&hex_of("background")),
        bg_light: rgba(&hex_of("lighter_background")),
        fg: rgba(&hex_of("foreground")),
        fg_dark: rgba(&hex_of("dark_foreground")),
    }
}

/// Publish `mod.sheets.*` so the splash below can read the live palette.
/// Call once, after `makepad_widgets::script_mod` and `mp_theme::apply`, and
/// before this crate's own `script_mod`.
pub fn install(vm: &mut ScriptVm) {
    let mut code = String::from("mod.sheets = {\n");
    for (key, _) in KEYS {
        // `background` -> `bg` reads better at the use site.
        let name = match *key {
            "background" => "bg",
            "darker_background" => "bg_dark",
            "lighter_background" => "bg_light",
            "foreground" => "fg",
            "bright_foreground" => "fg_bright",
            "dark_foreground" => "fg_dark",
            other => other,
        };
        code.push_str(&format!("    {name}: {}\n", hex_of(key)));
    }
    // Translucent variants: a selection fill has to let the cell text through.
    code.push_str(&format!("    sel_fill: {}66\n", hex_of("selection")));
    code.push_str(&format!("    accent_ghost: {}44\n", hex_of("accent")));
    code.push_str("}\n");

    // `mp_theme::apply` only retints the stock widgets when the WM exported a
    // palette. Standalone that leaves the caption bar and the drop-down popup
    // in the light stock theme, which looks broken next to a dark sheet — so
    // apply the same retint here, with the Tokyo Night fallbacks.
    let bg = hex_of("background");
    let bg_dark = hex_of("darker_background");
    let bg_light = hex_of("lighter_background");
    let fg = hex_of("foreground");
    let fg_bright = hex_of("bright_foreground");
    let fg_dark = hex_of("dark_foreground");
    let accent = hex_of("accent");
    let selection = hex_of("selection");
    let muted = hex_of("muted");
    code.push_str(&format!(
        "mod.theme.color_b = {bg_dark}\n\
         mod.theme.color_b_h = {bg_dark}00\n\
         mod.theme.color_w = {fg_bright}\n\
         mod.theme.color_w_h = {fg_bright}00\n\
         mod.theme.color_bg_app = {bg}\n\
         mod.theme.color_fg_app = {bg_light}\n\
         mod.theme.color_bg_container = {bg_dark}\n\
         mod.theme.color_text = {fg}\n\
         mod.theme.color_text_hover = {fg_bright}\n\
         mod.theme.color_text_muted = {fg_dark}\n\
         mod.theme.color_outset_active = {accent}\n\
         mod.theme.color_focus = {accent}\n\
         mod.theme.color_bg_highlight = {selection}\n\
         mod.theme.color_bg_highlight_inline = {muted}\n\
         mod.theme.color_bg_odd = {bg}\n\
         mod.theme.color_bg_even = {bg_light}\n\
         mod.theme.color_ctrl_default = {bg_light}\n\
         mod.theme.color_ctrl_hover = {muted}\n\
         mod.theme.color_ctrl_active = {accent}\n\
         mod.theme.color_ctrl_selected = {accent}\n\
         mod.theme.color_app_caption_bar = {bg_dark}\n\
         mod.theme.corner_radius = 0.0\n"
    ));
    code.push_str("true\n");

    let script_mod_id = ScriptMod {
        cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
        module_path: "mpsheets_theme".to_string(),
        file: "mpsheets_theme.splash".to_string(),
        line: 0,
        column: 0,
        code,
        values: vec![],
    };
    vm.eval(script_mod_id);
    for e in vm.take_errors() {
        log!("mpsheets theme: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing_matches_the_splash_tokenizer() {
        let c = rgba("#ff0000");
        assert!((c.x - 1.0).abs() < 1e-6 && c.y == 0.0 && c.z == 0.0 && c.w == 1.0);
        let c = rgba("#1a1b26");
        assert!((c.x - 26.0 / 255.0).abs() < 1e-6);
        assert!((c.y - 27.0 / 255.0).abs() < 1e-6);
        assert!((c.z - 38.0 / 255.0).abs() < 1e-6);
        // 8 digits carry alpha
        let c = rgba("#00000080");
        assert!((c.w - 128.0 / 255.0).abs() < 1e-6);
        // 3 digits expand
        let c = rgba("#fff");
        assert_eq!((c.x, c.y, c.z, c.w), (1.0, 1.0, 1.0, 1.0));
        // a leading '#' is optional
        assert_eq!(rgba("ff0000").x, 1.0);
    }

    #[test]
    fn every_key_has_a_usable_fallback() {
        for (key, fallback) in KEYS {
            assert!(fallback.starts_with('#'), "{key} fallback must be a hex colour");
            let c = rgba(fallback);
            assert_eq!(c.w, 1.0, "{key} fallback must be opaque");
        }
    }
}
