//! App-local appearance layered over the shared OS widget style. Switching
//! appearance re-evaluates Splash, preserving the live performance state.
use makepad_widgets::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Appearance {
    /// Follow the host when present; standalone consoles retain their design.
    #[default]
    Automatic,
    System,
    BlackOrange,
}

#[derive(Default)]
struct ThemeState {
    appearance: Appearance,
    compact: bool,
    custom: bool,
    touch: bool,
    generation: u64,
    colors: HashMap<LiveId, u32>,
}

pub fn is_custom(cx: &mut Cx) -> bool {
    cx.global::<ThemeState>().custom
}

pub fn generation(cx: &mut Cx) -> u64 {
    cx.global::<ThemeState>().generation
}

pub fn control_height(cx: &mut Cx) -> f64 {
    if cx.global::<ThemeState>().touch { 44.0 } else { 26.0 }
}

pub fn rgba(cx: &mut Cx, role: LiveId) -> u32 {
    cx.global::<ThemeState>().colors.get(&role).copied().unwrap_or(0xffffffff)
}

pub fn color(cx: &mut Cx, role: LiveId) -> Vec4f {
    Vec4f::from_u32(rgba(cx, role))
}

pub fn toggle(cx: &mut Cx) {
    let state = cx.global::<ThemeState>();
    state.appearance = if state.custom { Appearance::System } else { Appearance::BlackOrange };
    cx.request_style_reload();
}

pub fn set_compact(cx: &mut Cx, compact: bool) {
    let state = cx.global::<ThemeState>();
    if state.compact != compact {
        state.compact = compact;
        cx.request_style_reload();
    }
}

pub fn script_mod(vm: &mut ScriptVm) {
    let hosted = desktop_style::current_name(vm).is_some();
    let mobile = desktop_style::current_style(vm).mobile();
    let state = vm.cx_mut().global::<ThemeState>();
    let custom = match state.appearance {
        Appearance::Automatic => !hosted,
        Appearance::System => false,
        Appearance::BlackOrange => true,
    };
    state.custom = custom;
    let compact = state.compact || mobile;
    state.touch = compact;
    let name = if custom { "black-orange" } else { "system" };
    let bundled = if custom {
        include_str!("../resources/themes/black-orange.splash")
    } else {
        include_str!("../resources/themes/system.splash")
    };
    let source = {
        #[cfg(not(target_arch = "wasm32"))]
        { std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources/themes").join(format!("{name}.splash")))
            .unwrap_or_else(|_| bundled.into()) }
        #[cfg(target_arch = "wasm32")]
        { bundled.to_string() }
    };
    vm.eval(ScriptMod {
        cargo_manifest_path: env!("CARGO_MANIFEST_DIR").into(),
        module_path: "vj_appearance".into(),
        file: format!("resources/themes/{name}.splash"),
        line: 0, column: 0,
        code: format!("{source}\nmod.vj_theme.control_height = {}\nmod.vj_theme.label_size = {}\nmod.vj_theme.grid_cell_min = {}\ntrue\n",
            if compact { 44 } else { 26 }, if compact { 11 } else { 10 }, if compact { 44 } else { 0 }),
        values: vec![],
    });
    apply_control_defaults(vm, custom);
    let module = vm.module(id!(vj_theme));
    if !custom {
        if let Some(accent) = vm.bx.heap.value(module, id!(accent).into(), NoTrap).as_color() {
            let channel = |shift: u32| {
                let s = ((accent >> shift) & 255u32) as f64 / 255.0;
                if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
            };
            let light = 0.2126 * channel(24) + 0.7152 * channel(16) + 0.0722 * channel(8);
            let ink = if light > 0.179 { "#000000" } else { "#ffffff" };
            vm.eval(ScriptMod {
                cargo_manifest_path: env!("CARGO_MANIFEST_DIR").into(),
                module_path: "vj_accent_ink".into(), file: "vj_accent_ink.splash".into(),
                line: 0, column: 0,
                code: format!("mod.vj_theme.on_accent = {ink}\ntrue\n"), values: vec![],
            });
        }
    }
    let mut colors = HashMap::new();
    for role in [id!(background), id!(surface), id!(surface_raised), id!(inset),
        id!(control), id!(control_hover), id!(control_down), id!(text),
        id!(text_secondary), id!(text_muted), id!(text_hover), id!(border),
        id!(border_strong), id!(separator), id!(accent), id!(accent_hover),
        id!(on_accent), id!(selection),id!(waveform_background),id!(waveform_text),id!(waveform_grid)] {
        if let Some(value) = vm.bx.heap.value(module, role.into(), NoTrap).as_color() {
            colors.insert(role, value);
        }
    }
    let state = vm.cx_mut().global::<ThemeState>();
    state.colors = colors;
    state.generation = state.generation.wrapping_add(1);
}

/// Reusable controls and the custom console widgets share the same roles.
/// Keep the OS border geometry; the console's own appearance uses flat fills.
fn apply_control_defaults(vm: &mut ScriptVm, custom: bool) {
    let mut source = String::from("let vj = mod.vj_theme\n");
    for component in ["Button", "ButtonFlat", "ButtonIcon", "DropDown", "TextInput"] {
        let root = format!("mod.widgets.{component}");
        let field = component == "TextInput";
        for (state, role) in [("", if field { "inset" } else { "control" }),
            ("_hover", if field { "inset" } else { "control_hover" }),
            ("_focus", if field { "inset" } else { "control" }),
            ("_down", if field { "inset" } else { "control_down" }),
            ("_disabled", "surface")] {
            source.push_str(&format!("{root}.draw_bg.color{state} = vj.{role}\n{root}.draw_bg.color_2{state} = vj.{role}\n"));
            let ink = if state == "_disabled" { "text_muted" } else { "text" };
            source.push_str(&format!("{root}.draw_text.color{state} = vj.{ink}\n"));
            if custom {
                source.push_str(&format!("{root}.draw_bg.border_color{state} = vj.border\n{root}.draw_bg.border_color_2{state} = vj.border\n"));
            }
        }
        if custom {
            source.push_str(&format!("{root}.draw_bg.border_radius = vj.radius\n{root}.draw_bg.border_size = 1.0\n"));
        }
    }
    source.push_str("mod.widgets.ValueInput.draw_text.color = vj.text\nmod.widgets.ValueInput.draw_bg.color = vj.inset\nmod.widgets.ValueInput.draw_bg.border_color = vj.border\nmod.widgets.ValueInput.draw_bg.arrow_color = vj.text_secondary\nmod.widgets.ValueInput.height = vj.control_height\n");
    vm.eval(ScriptMod {
        cargo_manifest_path: env!("CARGO_MANIFEST_DIR").into(),
        module_path: "vj_controls".into(), file: "vj_controls.splash".into(),
        line: 0, column: 0, code: source, values: vec![],
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_custom_appearance_survives_host_style_changes() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(|vm| {
            makepad_widgets::makepad_platform::script::script_mod(vm);
            makepad_widgets::script_mod(vm);
            for style in desktop_style::DesktopStyle::ALL {
                desktop_style::install(vm, desktop_style::StyleSheet::load(style));
                vm.cx_mut().global::<ThemeState>().appearance = Appearance::BlackOrange;
                vm.bx.captured_errors = Some(Vec::new());
                vm.with_reload(|vm| { makepad_widgets::script_mod(vm); script_mod(vm); });
                assert!(vm.take_errors().is_empty());
                assert_eq!(rgba(vm.cx_mut(), id!(accent)), 0xff5c39ff);
                assert_eq!(rgba(vm.cx_mut(), id!(background)), 0x14171cff);
                vm.cx_mut().global::<ThemeState>().appearance = Appearance::System;
                vm.bx.captured_errors = Some(Vec::new());
                vm.with_reload(|vm| { makepad_widgets::script_mod(vm); script_mod(vm); });
                assert!(vm.take_errors().is_empty());
                let theme = vm.module(id!(theme));
                let background = vm.bx.heap.value(theme, id!(color_bg_app).into(), NoTrap).as_color().unwrap();
                assert_eq!(rgba(vm.cx_mut(), id!(background)), background);
            }
        });
    }
}
