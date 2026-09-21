//! The Makepad theme bridge: retint the makepad widgets theme from the WM's
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
            if key != "mod.wm_theme =" && !key.starts_with("mod.") {
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

/// The theme.splash the WM exported for this process, if any.
fn exported_theme_path() -> Option<String> {
    std::env::var("MAKEPAD_WM_THEME_SPLASH")
        .ok()
        .filter(|path| !path.is_empty())
}

fn read_exported_palette(path: &str) -> Option<Palette> {
    let source = std::fs::read_to_string(path).ok()?;
    let palette = scan(&source);
    (!palette.colors.is_empty()).then_some(palette)
}

/// The palette wm exported for this process, if any. Reads the theme file on
/// every call, so it is for registration-time and test use; draw paths read
/// [`current_for_vm`], which caches per isolate.
pub fn current() -> Option<Palette> {
    read_exported_palette(&exported_theme_path()?)
}

/// Per-isolate palette cache, a `Cx` global. An entry remembers the inputs it
/// was derived from — the installed stylesheet name and the exported theme
/// path — so a style switch, a light/dark toggle, or a re-exported theme
/// misses on its own, and [`apply`] drops the entry before retinting. Draw
/// paths therefore never touch the disk; the theme file is read once per
/// [`apply`] (or once per style change for isolates that never call it).
#[derive(Default)]
struct PaletteCache {
    heaps: HashMap<usize, CachedPalette>,
}

struct CachedPalette {
    style: Option<String>,
    path: Option<String>,
    palette: Option<Palette>,
}

/// Drop every cached palette. For hosts that change the exported theme
/// without re-running [`apply`] in each isolate.
pub fn invalidate(cx: &mut Cx) {
    cx.global::<PaletteCache>().heaps.clear();
}

/// The active Splash palette in this application's isolate. Custom app surfaces
/// use these semantic roles alongside stock widget styles.
///
/// Cached per isolate (see [`PaletteCache`]); a style change or a call to
/// [`apply`] refreshes it. Under the Omarchy style the palette is the exported
/// theme file (including `term.*` keys); otherwise it is derived from the
/// installed stylesheet's `mod.theme` roles.
pub fn current_for_vm(vm: &mut ScriptVm) -> Option<Palette> {
    let style = makepad_widgets::desktop_style::current_name(vm);
    let path = exported_theme_path();
    let key = vm.bx.heap.heap_key();
    if let Some(entry) = vm.cx_mut().global::<PaletteCache>().heaps.get(&key) {
        if entry.style == style && entry.path == path {
            return entry.palette.clone();
        }
    }
    let palette = resolve_palette(vm, style.as_deref(), path.as_deref());
    vm.cx_mut().global::<PaletteCache>().heaps.insert(
        key,
        CachedPalette {
            style,
            path,
            palette: palette.clone(),
        },
    );
    palette
}

fn resolve_palette(vm: &mut ScriptVm, style: Option<&str>, path: Option<&str>) -> Option<Palette> {
    if !style.is_some_and(|s| s != "omarchy") {
        return path.and_then(read_exported_palette);
    }
    let theme = vm.module(id!(theme));
    let light_mode = style.is_none_or(|s| !s.ends_with("-dark"));
    let mut p = Palette {
        light_mode,
        ..Default::default()
    };
    for (key, role) in [
        ("background", "color_bg_app"),
        ("darker_background", "color_fg_app"),
        ("dark_background", "color_bg_container"),
        ("lighter_background", "color_inset"),
        ("foreground", "color_text"),
        ("bright_foreground", "color_text_hover"),
        ("dark_foreground", "color_text_disabled"),
        ("accent", "color_focus"),
        ("selection", "color_bg_highlight"),
        ("muted", "color_bevel_outset_2"),
        ("term.background", "color_terminal_bg"),
        ("term.foreground", "color_terminal_text"),
    ] {
        if let Some(c) = vm
            .bx
            .heap
            .value(theme, LiveId::from_str(role).into(), NoTrap)
            .as_color()
        {
            p.colors.insert(key.into(), format!("#{:06x}", c >> 8));
        }
    }
    Some(p)
}

/// Retint `mod.theme` from the WM palette. Call once right after
/// `makepad_widgets::script_mod(vm)`, before the app's own script_mod; it is
/// re-run on every style reload, which also refreshes this isolate's cached
/// palette. No-op when MAKEPAD_WM_THEME_SPLASH is unset (standalone runs keep
/// the stock theme).
pub fn apply(vm: &mut ScriptVm) {
    let key = vm.bx.heap.heap_key();
    vm.cx_mut().global::<PaletteCache>().heaps.remove(&key);
    if makepad_widgets::desktop_style::current_name(vm).is_some_and(|s| s != "omarchy") {
        makepad_widgets::desktop_style::apply_widgets(vm);
        return;
    }
    let path = exported_theme_path();
    let Some(p) = path.as_deref().and_then(read_exported_palette) else {
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
    // The palette just read is the one draw paths will ask for.
    let style = makepad_widgets::desktop_style::current_name(vm);
    vm.cx_mut().global::<PaletteCache>().heaps.insert(
        key,
        CachedPalette {
            style,
            path,
            palette: Some(p),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_application_palette_follows_the_active_splash_style() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            desktop_style::install(vm,desktop_style::StyleSheet::load(desktop_style::DesktopStyle::Macos));
            vm.with_reload(makepad_widgets::widgets_mod);
            let p=current_for_vm(vm).unwrap();
            assert_eq!(p.get("background"),Some("#ececec"),"{:?}",p);
            assert_eq!(p.get("foreground"),Some("#242426"));
        });
    }

    #[test]
    fn cached_palette_follows_style_switches_and_apply() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            desktop_style::install(vm, desktop_style::StyleSheet::load(desktop_style::DesktopStyle::Macos));
            vm.with_reload(makepad_widgets::widgets_mod);
            assert_eq!(current_for_vm(vm).unwrap().get("background"), Some("#ececec"));
            // Same inputs: served from the cache, same answer.
            assert_eq!(current_for_vm(vm).unwrap().get("background"), Some("#ececec"));
            assert_eq!(vm.cx_mut().global::<PaletteCache>().heaps.len(), 1);
            // A dark toggle changes the style name, so the entry misses.
            desktop_style::install(vm, desktop_style::StyleSheet::load_with_appearance(desktop_style::DesktopStyle::Macos, true));
            vm.with_reload(makepad_widgets::widgets_mod);
            let p = current_for_vm(vm).unwrap();
            assert_eq!(p.get("background"), Some("#28282a"), "{:?}", p);
            assert!(!p.light_mode);
            // `apply` (re-run by every style reload) drops the entry and re-resolves.
            desktop_style::install(vm, desktop_style::StyleSheet::load(desktop_style::DesktopStyle::Windows));
            vm.with_reload(makepad_widgets::widgets_mod);
            apply(vm);
            assert!(vm.cx_mut().global::<PaletteCache>().heaps.is_empty());
            let p = current_for_vm(vm).unwrap();
            assert_eq!(p.get("background"), Some("#f3f3f3"), "{:?}", p);
            assert!(p.light_mode);
        });
    }

    #[test]
    fn terminal_colors_follow_the_style_without_changing_application_surfaces() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            for (style, dark, background, terminal) in [
                (desktop_style::DesktopStyle::Windows2000, false, "#d4d0c8", Some("#000000")),
                (desktop_style::DesktopStyle::NextStep, false, "#aaaaaa", Some("#ffffff")),
                (desktop_style::DesktopStyle::Android, false, "#fef7ff", Some("#000000")),
                (desktop_style::DesktopStyle::Android, true, "#141218", Some("#000000")),
                (desktop_style::DesktopStyle::Macos, false, "#ececec", Some("#ececec")),
            ] {
                desktop_style::install(vm, desktop_style::StyleSheet::load_with_appearance(style, dark));
                vm.with_reload(makepad_widgets::widgets_mod);
                apply(vm);
                let p = current_for_vm(vm).unwrap();
                assert_eq!(p.get("background"), Some(background), "{style:?}");
                assert_eq!(p.get("term.background"), terminal, "{style:?}");
            }
        });
    }

    #[test]
    fn omarchy_palette_reads_the_exported_theme_once_per_apply() {
        let path = std::env::temp_dir().join(format!(
            "makepad_wm_theme_{}_{:?}.splash",
            std::process::id(),
            std::thread::current().id()
        ));
        let write = |accent: &str| {
            std::fs::write(
                &path,
                format!("mod.wm_theme = {{\n    accent: {accent}\n    background: #1a1b26\n    term: {{\n        color1: #f7768e\n    }}\n}}\n"),
            )
            .unwrap();
        };
        write("#7aa2f7");
        std::env::set_var("MAKEPAD_WM_THEME_SPLASH", &path);
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            // The WM installs Omarchy in every hosted app: the palette is the exported file.
            desktop_style::install(vm, desktop_style::StyleSheet::load(desktop_style::DesktopStyle::Omarchy));
            let p = current_for_vm(vm).unwrap();
            assert_eq!(p.get("accent"), Some("#7aa2f7"));
            assert_eq!(p.get("term.color1"), Some("#f7768e"));
            // The file changing underneath does not reach draw paths by itself...
            write("#ff9e64");
            assert_eq!(current_for_vm(vm).unwrap().get("accent"), Some("#7aa2f7"));
            // ...`apply` (every style reload) re-reads it, once, and retints the theme.
            apply(vm);
            assert_eq!(current_for_vm(vm).unwrap().get("accent"), Some("#ff9e64"));
            let theme = vm.module(id!(theme));
            assert_eq!(
                vm.bx.heap.value(theme, id!(color_focus).into(), NoTrap).as_color(),
                Some(0xff9e64ff)
            );
            // An explicit invalidation re-reads too.
            write("#9ece6a");
            assert_eq!(current_for_vm(vm).unwrap().get("accent"), Some("#ff9e64"));
            invalidate(vm.cx_mut());
            assert_eq!(current_for_vm(vm).unwrap().get("accent"), Some("#9ece6a"));
        });
        std::env::remove_var("MAKEPAD_WM_THEME_SPLASH");
        let _ = std::fs::remove_file(&path);
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
}
