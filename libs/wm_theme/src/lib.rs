//! The Makepad theme bridge: retint the makepad widgets theme from the WM's
//! theme.splash (see Cargo.toml): base black/white, app bg/fg, accent,
//! selection, text, and the material's corner radius. Theming LIVES in
//! splash — this crate only ferries the WM's palette into `mod.theme`. The
//! stock widget types captured `theme.*` when the widgets module was
//! defined, before this bridge runs, so these assignments reach an app's own
//! DSL (evaluated after `apply`) and anything reading `mod.theme` later —
//! not the stock widget prototypes themselves (Window's clear color is
//! patched directly for that reason).

use makepad_widgets::*;
use std::collections::HashMap;

/// The palette scanned from a theme.splash (`key: #hex` lines).
#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub colors: HashMap<String, String>,
    pub light_mode: bool,
    /// material.corner_radius, visual px; 0 for every flat theme.
    pub corner_radius: f64,
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
        // As apps/wm's scan_values: a trailing `// comment` is dropped, then
        // the trailing comma.
        let value = value.split("//").next().unwrap_or("").trim().trim_end_matches(',').trim();
        if value == "{" {
            if key != "mod.wm_theme =" && !key.starts_with("mod.") {
                prefix.push(key.to_string());
            }
            continue;
        }
        if key == "material" && prefix.is_empty() && value.starts_with('{') && value.ends_with('}') {
            // The one-line block form: `material: { glass: 1.0, corner_radius: 12.0 }`.
            let inner = &value[1..value.len() - 1];
            for pair in inner.split(',') {
                if let Some((k, v)) = pair.split_once(':') {
                    if k.trim() == "corner_radius" {
                        palette.corner_radius = radius(v);
                    }
                }
            }
            continue;
        }
        if key == "light_mode" {
            palette.light_mode = value == "true";
            continue;
        }
        if key == "corner_radius" && prefix.len() == 1 && prefix[0] == "material" {
            palette.corner_radius = radius(value);
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

/// A material radius value: finite and non-negative, else 0.
fn radius(value: &str) -> f64 {
    value.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0)
}

/// The palette wm exported for this process, if any.
pub fn current() -> Option<Palette> {
    let path = std::env::var("MAKEPAD_WM_THEME_SPLASH").ok()?;
    let source = std::fs::read_to_string(path).ok()?;
    let palette = scan(&source);
    (!palette.colors.is_empty()).then_some(palette)
}

/// Retint `mod.theme` from the WM palette. Call once right after
/// `makepad_widgets::script_mod(vm)`, before the app's own script_mod.
/// No-op when MAKEPAD_WM_THEME_SPLASH is unset (standalone runs keep the stock
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
    // The widgets theme's radius is the Sdf2d.box argument, which renders at
    // twice its value (stock 2.5 → 5px corners); the material's radius is
    // visual px, so halve it.
    let radius = p.corner_radius * 0.5;
    let corner_radius = format!("{:.1}", radius);
    // The stock theme derives these two from corner_radius at definition
    // time; keep mod.theme self-consistent for whatever reads it after this
    // runs.
    let container_corner_radius = format!("{:.1}", radius * 2.0);
    let textselection_corner_radius = format!("{:.1}", radius * 0.5);

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
         mod.theme.corner_radius = {corner_radius}\n\
         mod.theme.container_corner_radius = {container_corner_radius}\n\
         mod.theme.textselection_corner_radius = {textselection_corner_radius}\n\
         true\n"
    );
    let script_mod_id = ScriptMod {
        cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
        module_path: "makepad_wm_theme".to_string(),
        file: "makepad_wm_theme.splash".to_string(),
        line: 0,
        column: 0,
        code,
        values: vec![],
    };
    vm.eval(script_mod_id);
    // Unknown keys on a given widgets version are harmless: the assignment
    // just creates them. Real errors (syntax) get logged.
    for e in vm.take_errors() {
        log!("makepad_wm_theme: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_script::{ScriptValue, ScriptVmBase, ScriptVmHost};

    /// A bare VM, built the way apps/wm's theme tests build one.
    fn test_vm() -> ScriptVm<'static> {
        let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
        ScriptVm {
            host,
            bx: Box::new(ScriptVmBase::new()),
        }
    }

    /// Evaluate `code` in `vm` as the module block `name`.
    fn eval(vm: &mut ScriptVm, name: &str, code: &str) -> ScriptValue {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: name.to_string(),
            file: format!("{name}.splash"),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    }

    /// The value of `expr` in `vm`: a probe block whose result expression
    /// it is.
    fn read(vm: &mut ScriptVm, expr: &str) -> ScriptValue {
        eval(vm, "probe", &format!("({expr})\n"))
    }

    #[test]
    fn scans_nested_theme() {
        let src = "// c\nmod.wm_theme = {\n    accent: #7aa2f7\n    background: #1a1b26\n    light_mode: false\n    term: {\n        color0: #1a1b26\n        cursor: #c0caf5\n    }\n}\n";
        let p = scan(src);
        assert_eq!(p.get("accent"), Some("#7aa2f7"));
        assert_eq!(p.get("background"), Some("#1a1b26"));
        assert_eq!(p.get("term.color0"), Some("#1a1b26"));
        assert_eq!(p.get("term.cursor"), Some("#c0caf5"));
        assert!(!p.light_mode);
        assert_eq!(p.hex("missing", "#000000"), "#000000");
    }

    #[test]
    fn scans_the_material_radius() {
        let src = "mod.wm_theme = {\n    accent: #7aa2f7\n    material: {\n        glass: 1.0\n        corner_radius: 12.0\n    }\n}\n";
        let p = scan(src);
        assert_eq!(p.corner_radius, 12.0);
        assert_eq!(p.get("accent"), Some("#7aa2f7"));
        assert_eq!(scan("mod.wm_theme = {\n    accent: #7aa2f7\n}\n").corner_radius, 0.0);
        // The one-line block form, a trailing comment, and the value filter.
        assert_eq!(scan("mod.wm_theme = {\n    material: { glass: 1.0, corner_radius: 12 }\n}\n").corner_radius, 12.0);
        assert_eq!(scan("mod.wm_theme = {\n    material: {\n        corner_radius: 8.0, // px\n    }\n}\n").corner_radius, 8.0);
        for bad in ["-1", "inf", "abc"] {
            let src = format!("mod.wm_theme = {{\n    material: {{\n        corner_radius: {bad}\n    }}\n}}\n");
            assert_eq!(scan(&src).corner_radius, 0.0, "{bad}");
        }
        // Not the desk's, and not the shell's.
        let other = "mod.wm_theme = {\n    desk: {\n        corner_radius: 12.0\n    }\n}\nmod.wm_theme.shell = {\n    corner_radius: 9.0\n}\n";
        assert_eq!(scan(other).corner_radius, 0.0);
    }

    #[test]
    fn apply_retints_the_radius_from_the_theme_file() {
        let mut vm = test_vm();
        // The stock theme and the one widget prototype `apply` patches.
        eval(
            &mut vm,
            "stock",
            "mod.theme = {\n    corner_radius: 2.5\n}\nmod.widgets = {\n    Window: {\n        pass: {}\n    }\n}\ntrue\n",
        );
        assert_eq!(read(&mut vm, "mod.theme.corner_radius").as_f64(), Some(2.5));
        let path = std::env::temp_dir().join(format!("makepad_wm_theme_{}.splash", std::process::id()));
        std::fs::write(
            &path,
            "mod.wm_theme = {\n    accent: #7aa2f7\n    material: {\n        corner_radius: 12.0\n    }\n}\n",
        )
        .unwrap();
        std::env::set_var("MAKEPAD_WM_THEME_SPLASH", &path);
        apply(&mut vm);
        std::env::remove_var("MAKEPAD_WM_THEME_SPLASH");
        let _ = std::fs::remove_file(&path);
        // 12 visual px is a 6.0 Sdf2d radius; the derived keys follow it.
        assert_eq!(read(&mut vm, "mod.theme.corner_radius").as_f64(), Some(6.0));
        assert_eq!(read(&mut vm, "mod.theme.container_corner_radius").as_f64(), Some(12.0));
        assert_eq!(read(&mut vm, "mod.theme.textselection_corner_radius").as_f64(), Some(3.0));
    }
}
