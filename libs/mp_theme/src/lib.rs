//! The mp* theme bridge: retint the makepad widgets theme from the WM's
//! theme.splash (see Cargo.toml). Theming LIVES in splash — this crate only
//! ferries the WM's palette into `mod.theme` so stock widgets follow it.

use makepad_widgets::*;
use std::collections::HashMap;

/// The palette scanned from a theme.splash (`key: #hex` lines).
#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub colors: HashMap<String, String>,
    pub light_mode: bool,
}

impl Palette {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.colors.get(key).map(|s| s.as_str())
    }

    /// Hex string for `key`, or `fallback`.
    pub fn hex(&self, key: &str, fallback: &str) -> String {
        self.get(key).unwrap_or(fallback).to_string()
    }
}

/// Line-scan a theme.splash source: `    accent: #7aa2f7` → ("accent",
/// "#7aa2f7"). Nested blocks (`term: {`) are scanned too; their keys are
/// prefixed (`term.color0`).
pub fn scan(source: &str) -> Palette {
    let mut palette = Palette::default();
    let mut prefix: Vec<String> = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with('}') {
            prefix.pop();
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_end_matches(',');
        if value == "{" {
            if key != "mod.mpwm_theme =" && !key.starts_with("mod.") {
                prefix.push(key.to_string());
            }
            continue;
        }
        if key == "light_mode" {
            palette.light_mode = value == "true";
            continue;
        }
        if value.starts_with('#') {
            let full = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{}.{}", prefix.join("."), key)
            };
            palette.colors.insert(full, value.to_string());
        }
    }
    palette
}

/// The palette mpwm exported for this process, if any.
pub fn current() -> Option<Palette> {
    let path = std::env::var("MPWM_THEME_SPLASH").ok()?;
    let source = std::fs::read_to_string(path).ok()?;
    let palette = scan(&source);
    (!palette.colors.is_empty()).then_some(palette)
}

/// Retint `mod.theme` from the WM palette. Call once right after
/// `makepad_widgets::script_mod(vm)`, before the app's own script_mod.
/// No-op when MPWM_THEME_SPLASH is unset (standalone runs keep the stock
/// theme).
pub fn apply(vm: &mut ScriptVm) {
    let Some(p) = current() else {
        return;
    };
    let bg = p.hex("background", "#1a1b26");
    let bg_dark = p.hex("darker_background", "#0e0e14");
    let bg_light = p.hex("lighter_background", "#24283b");
    let fg = p.hex("foreground", "#a9b1d6");
    let fg_bright = p.hex("bright_foreground", "#c0caf5");
    let fg_dark = p.hex("dark_foreground", "#565f89");
    let accent = p.hex("accent", "#7aa2f7");
    let selection = p.hex("selection", "#292e42");
    let muted = p.hex("muted", "#414868");

    // The widgets theme derives its whole ladder from color_b/color_w and
    // the app bg/fg; overriding those (plus the handful of named roles apps
    // reach for directly) retints stock widgets without touching them.
    // Omarchy's controls are flat: one fill and one 1px border per state.
    // The stock theme paints every inset (text fields) and outset (buttons)
    // as a two-stop gradient — pair 1/2 keys — so both stops get the same
    // color here, per state. The derived keys were evaluated when the theme
    // was defined, so every state is spelled out.
    let mut flat = String::new();
    let field = &bg_light;
    for state in ["", "_hover", "_down", "_active", "_focus", "_drag", "_empty"] {
        flat.push_str(&format!(
            "mod.theme.color_inset_1{state} = {field}\nmod.theme.color_inset_2{state} = {field}\n"
        ));
    }
    flat.push_str(&format!(
        "mod.theme.color_inset_1_disabled = {bg}\nmod.theme.color_inset_2_disabled = {bg}\n"
    ));
    for state in ["", "_hover", "_empty", "_drag"] {
        flat.push_str(&format!(
            "mod.theme.color_bevel_inset_1{state} = {muted}\nmod.theme.color_bevel_inset_2{state} = {muted}\n"
        ));
    }
    for state in ["_focus", "_active", "_down"] {
        flat.push_str(&format!(
            "mod.theme.color_bevel_inset_1{state} = {accent}\nmod.theme.color_bevel_inset_2{state} = {accent}\n"
        ));
    }
    flat.push_str(&format!(
        "mod.theme.color_bevel_inset_1_disabled = {bg}\nmod.theme.color_bevel_inset_2_disabled = {bg}\n"
    ));
    for state in ["", "_hover", "_focus", "_active", "_drag"] {
        flat.push_str(&format!(
            "mod.theme.color_bevel_outset_1{state} = {muted}\nmod.theme.color_bevel_outset_2{state} = {muted}\n"
        ));
    }
    // Buttons: FLAT blueish fills — both gradient stops identical per
    // state (the stock two-stop bevel look reads as "terrible gradient
    // buttons"). Idle = the theme's raised surface, hover = muted, down/
    // active = the accent (text stays readable via color_text roles).
    for (state, fill) in [
        ("", field),
        ("_hover", &muted),
        ("_down", &accent),
        ("_active", &accent),
        ("_focus", field),
        ("_drag", &muted),
    ] {
        flat.push_str(&format!(
            "mod.theme.color_outset_1{state} = {fill}\nmod.theme.color_outset_2{state} = {fill}\n"
        ));
    }
    flat.push_str(&format!(
        "mod.theme.color_outset_1_disabled = {bg}\nmod.theme.color_outset_2_disabled = {bg}\n"
    ));
    flat.push_str(&format!(
        "mod.theme.color_bevel_outset_1_down = {accent}\nmod.theme.color_bevel_outset_2_down = {accent}\n\
         mod.theme.color_bevel_outset_1_disabled = {bg}\nmod.theme.color_bevel_outset_2_disabled = {bg}\n"
    ));

    // Window's pass clear color was captured from theme.color_bg_app when
    // the widgets module was evaluated — before this retint — so an app that
    // never sets clear_color itself would clear to the stock neutral gray.
    let code = format!(
        "{flat}\
         mod.widgets.Window.pass.clear_color = {bg}\n\
         mod.theme.color_b = {bg_dark}\n\
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
         mod.theme.corner_radius = 0.0\n\
         true\n"
    );
    let script_mod_id = ScriptMod {
        cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
        module_path: "mp_theme".to_string(),
        file: "mp_theme.splash".to_string(),
        line: 0,
        column: 0,
        code,
        values: vec![],
    };
    vm.eval(script_mod_id);
    // Unknown keys on a given widgets version are harmless: the assignment
    // just creates them. Real errors (syntax) get logged.
    for e in vm.take_errors() {
        log!("mp_theme: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_nested_theme() {
        let src = "// c\nmod.mpwm_theme = {\n    accent: #7aa2f7\n    background: #1a1b26\n    light_mode: false\n    term: {\n        color0: #1a1b26\n        cursor: #c0caf5\n    }\n}\n";
        let p = scan(src);
        assert_eq!(p.get("accent"), Some("#7aa2f7"));
        assert_eq!(p.get("background"), Some("#1a1b26"));
        assert_eq!(p.get("term.color0"), Some("#1a1b26"));
        assert_eq!(p.get("term.cursor"), Some("#c0caf5"));
        assert!(!p.light_mode);
        assert_eq!(p.hex("missing", "#000000"), "#000000");
    }
}
