//! Hotloadable application styles. The WM owns framebuffer transitions; this
//! module only installs Splash definitions and reapplies the existing widget tree.
use crate::*;
use makepad_micro_serde::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DesktopStyle {
    #[default]
    Omarchy,
    Macos,
    Windows,
    Windows2000,
    NextStep,
    Ios,
    Android,
}

impl DesktopStyle {
    pub const ALL: [Self; 7] = [Self::Omarchy, Self::Macos, Self::Windows, Self::Windows2000, Self::NextStep, Self::Ios, Self::Android];
    pub fn id(self) -> &'static str {
        match self {
            Self::Omarchy => "omarchy",
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Windows2000 => "windows-2000",
            Self::NextStep => "nextstep",
            Self::Ios => "ios",
            Self::Android => "android",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Omarchy => "Omarchy",
            Self::Macos => "macOS",
            Self::Windows => "Windows",
            Self::Windows2000 => "Windows 2000",
            Self::NextStep => "NeXTSTEP",
            Self::Ios => "iOS",
            Self::Android => "Android",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_suffix("-dark").unwrap_or(s);
        Self::ALL.into_iter().find(|v| v.id() == s)
    }
    pub fn supports_dark(self) -> bool { matches!(self, Self::Macos | Self::Windows | Self::Ios | Self::Android) }
    pub fn mobile(self) -> bool { matches!(self, Self::Ios | Self::Android) }
    pub fn next(self) -> Self {
        Self::ALL[(self as usize + 1) % Self::ALL.len()]
    }
    pub fn floating(self) -> bool {
        self != Self::Omarchy && !self.mobile()
    }
    pub fn shelf_height(self) -> f64 {
        match self {
            Self::Omarchy => 0.0,
            Self::Macos => 86.0,
            Self::Windows => 54.0,
            Self::Windows2000 => 34.0,
            Self::NextStep | Self::Ios | Self::Android => 0.0,
        }
    }
    pub fn title_height(self) -> f64 {
        match self {
            Self::Omarchy => 0.0,
            Self::Macos => 32.0,
            Self::Windows => 34.0,
            Self::Windows2000 => 24.0,
            Self::NextStep => 26.0,
            Self::Ios | Self::Android => 0.0,
        }
    }
}

/// Two phases: theme tokens before widget registration, component overrides
/// after it. Applications then evaluate their own Splash against those defaults.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct StyleSheet {
    pub name: String,
    pub theme: String,
    pub widgets: String,
    pub icons: Vec<crate::app_icon::IconAsset>,
}
#[derive(SerJson, DeJson)]
struct Envelope {
    makepad_style: StyleSheet,
}
#[derive(Default)]
struct Styles {
    heaps: HashMap<usize, StyleSheet>,
}

impl StyleSheet {
    pub fn load(style: DesktopStyle) -> Self {
        Self::load_with_appearance(style, false)
    }
    pub fn load_with_appearance(style: DesktopStyle, dark: bool) -> Self {
        let name = match (style, dark) {
            (DesktopStyle::Macos, true) => "macos-dark",
            (DesktopStyle::Windows, true) => "windows-dark",
            (DesktopStyle::Ios, true) => "ios-dark",
            (DesktopStyle::Android, true) => "android-dark",
            _ => style.id(),
        };
        let (theme, widgets) = match style {
            DesktopStyle::Omarchy => (
                include_str!("../themes/omarchy/theme.splash"),
                include_str!("../themes/omarchy/widgets.splash"),
            ),
            DesktopStyle::Macos if dark => (
                include_str!("../themes/macos-dark/theme.splash"),
                include_str!("../themes/macos-dark/widgets.splash"),
            ),
            DesktopStyle::Macos => (
                include_str!("../themes/macos/theme.splash"),
                include_str!("../themes/macos/widgets.splash"),
            ),
            DesktopStyle::Windows if dark => (
                include_str!("../themes/windows-dark/theme.splash"),
                include_str!("../themes/windows-dark/widgets.splash"),
            ),
            DesktopStyle::Windows => (
                include_str!("../themes/windows/theme.splash"),
                include_str!("../themes/windows/widgets.splash"),
            ),
            DesktopStyle::Windows2000 => (
                include_str!("../themes/windows-2000/theme.splash"),
                include_str!("../themes/windows-2000/widgets.splash"),
            ),
            DesktopStyle::NextStep => (
                include_str!("../themes/nextstep/theme.splash"),
                include_str!("../themes/nextstep/widgets.splash"),
            ),
            DesktopStyle::Ios if dark => (
                include_str!("../themes/ios-dark/theme.splash"),
                include_str!("../themes/ios-dark/widgets.splash"),
            ),
            DesktopStyle::Ios => (
                include_str!("../themes/ios/theme.splash"),
                include_str!("../themes/ios/widgets.splash"),
            ),
            DesktopStyle::Android if dark => (
                include_str!("../themes/android-dark/theme.splash"),
                include_str!("../themes/android-dark/widgets.splash"),
            ),
            DesktopStyle::Android => (
                include_str!("../themes/android/theme.splash"),
                include_str!("../themes/android/widgets.splash"),
            ),
        };
        let read = |file: &str, bundled: &str| {
            // Source checkouts hotload on every selection; installed/wasm builds
            // carry the identical embedded stylesheet, with no external dependency.
            #[cfg(not(target_arch = "wasm32"))]
            if let Ok(text) = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("themes")
                    .join(name)
                    .join(file),
            ) {
                return text;
            }
            let _ = file;
            bundled.to_string()
        };
        Self {
            name: name.into(),
            theme: read("theme.splash", theme),
            widgets: read("widgets.splash", widgets),
            icons: crate::app_icon::load_assets(style),
        }
    }
    pub fn to_json(&self) -> String {
        Envelope {
            makepad_style: self.clone(),
        }
        .serialize_json()
    }
    pub fn parse(json: &str) -> Option<Self> {
        if !json.contains("\"makepad_style\"") {
            return None;
        }
        Envelope::deserialize_json(json)
            .ok()
            .map(|e| e.makepad_style)
    }
}

pub(crate) fn gc_heaps(cx: &mut Cx, heaps: &[usize]) {
    cx.global::<Styles>()
        .heaps
        .retain(|key, _| !heaps.contains(key));
}
pub fn install(vm: &mut ScriptVm, sheet: StyleSheet) {
    let style = DesktopStyle::parse(&sheet.name).unwrap_or(DesktopStyle::Macos);
    crate::app_icon::install(vm.cx_mut(), style, &sheet.icons);
    let key = vm.bx.heap.heap_key();
    vm.cx_mut().global::<Styles>().heaps.insert(key, sheet);
}
pub fn current(vm: &mut ScriptVm) -> Option<StyleSheet> {
    let key = vm.bx.heap.heap_key();
    if let Some(sheet) = vm.cx_mut().global::<Styles>().heaps.get(&key).cloned() {
        return Some(sheet);
    }
    let name = std::env::var("MAKEPAD_WIDGET_STYLE").ok().or_else(|| match vm.cx().os_type() {
        OsType::Ios(_) => Some("ios".into()),
        OsType::Android(_) => Some("android".into()),
        _ => None,
    })?;
    let style = DesktopStyle::parse(&name)?;
    let sheet = StyleSheet::load_with_appearance(style, name.ends_with("-dark"));
    install(vm, sheet.clone());
    Some(sheet)
}
/// Read just the active appearance without cloning the Splash and SVG payloads.
pub fn current_name(vm: &mut ScriptVm) -> Option<String> {
    let key = vm.bx.heap.heap_key();
    if let Some(sheet) = vm.cx_mut().global::<Styles>().heaps.get(&key) {
        return Some(sheet.name.clone());
    }
    current(vm).map(|sheet| sheet.name)
}
/// The active family, without copying a stylesheet or its assets.
pub fn current_style(vm: &mut ScriptVm) -> DesktopStyle {
    let key=vm.bx.heap.heap_key();
    if let Some(sheet)=vm.cx_mut().global::<Styles>().heaps.get(&key) {
        return DesktopStyle::parse(&sheet.name).unwrap_or_default();
    }
    current(vm).and_then(|sheet|DesktopStyle::parse(&sheet.name)).unwrap_or_default()
}
fn evaluate(vm: &mut ScriptVm, sheet: &StyleSheet, phase: &str, code: String) {
    vm.eval(ScriptMod {
        cargo_manifest_path: env!("CARGO_MANIFEST_DIR").into(),
        module_path: format!("desktop_style_{}_{}", sheet.name, phase),
        file: format!("themes/{}/{phase}.splash", sheet.name),
        line: 0,
        column: 0,
        code,
        values: vec![],
    });
}
pub fn apply_theme(vm: &mut ScriptVm) {
    if let Some(sheet) = current(vm) {
        evaluate(vm, &sheet, "theme", sheet.theme.clone());
        if DesktopStyle::parse(&sheet.name).is_some_and(|style| style.mobile()) {
            crate::font_policy::append_style_fallbacks(vm);
        }
    }
}
pub fn apply_widgets(vm: &mut ScriptVm) {
    if let Some(sheet) = current(vm) {
        evaluate(vm, &sheet, "widgets", sheet.widgets.clone());
    }
}
/// Window provides the common receive path, including apps with no WM API dependency.
pub fn handle_event(cx: &mut Cx, event: &Event) {
    let Event::Custom(json) = event else {
        return;
    };
    let Some(sheet) = StyleSheet::parse(json) else {
        return;
    };
    let changed = cx.with_vm(|vm| {
        if current(vm).as_ref() == Some(&sheet) {
            return false;
        }
        install(vm, sheet);
        true
    });
    if changed {
        cx.request_style_reload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mobile_typefaces_keep_symbol_fallbacks_across_appearances() {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        cx.init_cx_os();
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            for style in [DesktopStyle::Ios,DesktopStyle::Android] {
                for dark in [false,true] {
                    install(vm,StyleSheet::load_with_appearance(style,dark));
                    vm.with_reload(crate::script_mod);
                    let value=script_eval!(vm,{mod.theme.font_regular});
                    let text=TextStyle::script_from_value(vm,value);
                    let members=text.font_family.member_ids().collect::<Vec<_>>();
                    assert_eq!(members.first(),Some(&"latin"));
                    assert!(members.contains(&"jetbrains_ui_symbols"),"{members:?}");
                    assert!(vm.take_errors().is_empty());
                }
            }
        });
    }
    #[test]
    fn styles_re_evaluate_splash_without_replacing_user_text() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let value = script_eval!(vm,{use mod.widgets.* Label{text:"original"}});
            let mut label = Label::script_from_value(vm, value);
            label.set_text(vm.cx_mut(), "edited document");
            for style in DesktopStyle::ALL {
                install(vm, StyleSheet::load(style));
                vm.with_reload(crate::script_mod);
                let errors = vm.take_errors();
                assert!(errors.is_empty(), "{}: {:?}", style.id(), errors);
                let theme = vm.module(id!(theme));
                let radius = vm
                    .bx
                    .heap
                    .value(theme, id!(corner_radius).into(), NoTrap)
                    .as_f64()
                    .unwrap();
                assert_eq!(
                    radius,
                    match style {
                        DesktopStyle::Macos => 6.0,
                        DesktopStyle::Windows => 4.0,
                        DesktopStyle::Ios => 14.0,
                        DesktopStyle::Android => 20.0,
                        _ => 0.0,
                    }
                );
                let value = script_eval!(vm,{use mod.widgets.* Label{text:"original"}});
                label.script_apply(vm, &Apply::ScriptReapply, &mut Scope::empty(), value);
                assert_eq!(label.text(), "edited document");
            }
        });
    }
    #[test]
    fn modern_dark_modes_hotload_component_geometry_and_tokens() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            for (style, dark) in [DesktopStyle::Macos, DesktopStyle::Windows, DesktopStyle::Ios, DesktopStyle::Android].into_iter().flat_map(|s| [false, true, false].map(|d|(s,d))) {
                install(
                    vm,
                    StyleSheet::load_with_appearance(style, dark),
                );
                vm.bx.captured_errors = Some(Vec::new());
                vm.with_reload(crate::script_mod);
                assert!(vm.take_errors().is_empty());
                let theme = vm.module(id!(theme));
                assert_eq!(
                    vm.bx
                        .heap
                        .value(theme, id!(color_bg_app).into(), NoTrap)
                        .as_color(),
                    Some(match (style, dark) {
                        (DesktopStyle::Macos, true) => 0x28282aff, (DesktopStyle::Macos, false) => 0xecececff,
                        (DesktopStyle::Ios, true) => 0x000000ff, (DesktopStyle::Ios, false) => 0xf2f2f7ff,
                        (DesktopStyle::Android, true) => 0x141218ff, (DesktopStyle::Android, false) => 0xfef7ffff,
                        (_, true) => 0x202020ff, (_, false) => 0xf3f3f3ff
                    })
                );
                let widgets = vm.module(id!(widgets));
                let button = vm
                    .bx
                    .heap
                    .value(widgets, id!(Button).into(), NoTrap)
                    .as_object()
                    .unwrap();
                let draw = vm
                    .bx
                    .heap
                    .value(button, id!(draw_bg).into(), NoTrap)
                    .as_object()
                    .unwrap();
                assert_eq!(
                    vm.bx
                        .heap
                        .value(draw, id!(border_radius).into(), NoTrap)
                        .as_f64(),
                    Some(match style {DesktopStyle::Macos=>3.0,DesktopStyle::Ios=>22.0,DesktopStyle::Android=>24.0,_=>4.0})
                );
            }
        });
    }
    #[test]
    fn style_reapply_preserves_panel_visibility() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let original = script_eval!(vm, {use mod.widgets.* View{visible: false}});
            let mut panel = View::script_from_value(vm, original);
            panel.visible = true;
            install(vm, StyleSheet::load(DesktopStyle::Windows));
            vm.with_reload(crate::script_mod);
            panel.script_apply(vm, &Apply::ScriptReapply, &mut Scope::empty(), original);
            assert!(panel.visible, "An open panel must survive a style change");
            panel.script_apply(vm, &Apply::Eval, &mut Scope::empty(), original);
            assert!(!panel.visible, "Explicit visibility edits must still apply");
        });
    }
    #[test]
    fn stylesheet_wire_preserves_both_splash_phases() {
        let sheet = StyleSheet::load(DesktopStyle::Windows2000);
        assert_eq!(StyleSheet::parse(&sheet.to_json()), Some(sheet));
        assert!(StyleSheet::parse("{\"wm\":\"Adopted\"}").is_none());
    }
}
