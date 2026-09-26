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
    /// A dark style in near-black and orange, the only one here not modelled
    /// on somebody else's desktop. Declared last: the window manager's style
    /// tween reads its weights by discriminant (1 is macOS, 3 Windows 2000,
    /// 4 NeXTSTEP), so a new style takes the next number and `ALL` below
    /// keeps the order they are shown in.
    BlackOrange,
    /// Soft moulded surfaces on one near-white ground: no borders, every
    /// visible edge is light on a shoulder. The first sheet built on the
    /// surface material.
    Neumorphic,
    /// Moulded grey plastic: caps standing off a warm grey housing under a
    /// hard light, wells cut into it. A press deepens.
    Molded,
    /// Black glossy plastic with a cyan indicator: a gloss sweep on every
    /// cap, and a press that lights up rather than moves.
    Glossy,
    /// Milled near-black metal lit from inside in orange: flat machined
    /// faces, a hard hairline on every edge, everything that is on glows.
    Milled,
}

impl DesktopStyle {
    pub const ALL: [Self; 12] = [Self::Omarchy, Self::BlackOrange, Self::Neumorphic, Self::Molded, Self::Glossy, Self::Milled, Self::Macos, Self::Windows, Self::Windows2000, Self::NextStep, Self::Ios, Self::Android];
    pub fn id(self) -> &'static str {
        match self {
            Self::Omarchy => "omarchy",
            Self::BlackOrange => "black-orange",
            Self::Neumorphic => "neumorphic",
            Self::Molded => "molded",
            Self::Glossy => "glossy",
            Self::Milled => "milled",
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
            Self::BlackOrange => "Black orange",
            Self::Neumorphic => "Neumorphic",
            Self::Molded => "Molded",
            Self::Glossy => "Glossy",
            Self::Milled => "Milled",
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
    /// Which set of app artwork this style draws, as an index into the icon
    /// table. A style is free to borrow another's drawings rather than have
    /// every icon redrawn for it -- the table is one entry per SET, not one
    /// per style, so the enum's own order must not be read as an index into
    /// it.
    pub fn icon_set(self) -> usize {
        match self {
            Self::Omarchy | Self::BlackOrange | Self::Neumorphic | Self::Glossy | Self::Milled => 0,
            Self::Macos => 1,
            Self::Windows | Self::Molded => 2,
            Self::Windows2000 => 3,
            Self::NextStep => 4,
            Self::Ios => 5,
            Self::Android => 6,
        }
    }
    pub fn next(self) -> Self {
        // By place in `ALL`, not by discriminant: the two orders differ.
        let at = Self::ALL.iter().position(|style| *style == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
    pub fn floating(self) -> bool {
        !matches!(self, Self::Omarchy | Self::BlackOrange | Self::Neumorphic | Self::Molded | Self::Glossy | Self::Milled) && !self.mobile()
    }
    pub fn shelf_height(self) -> f64 {
        match self {
            Self::Omarchy | Self::BlackOrange | Self::Neumorphic | Self::Molded | Self::Glossy | Self::Milled => 0.0,
            Self::Macos => 86.0,
            Self::Windows => 54.0,
            Self::Windows2000 => 34.0,
            Self::NextStep | Self::Ios | Self::Android => 0.0,
        }
    }
    pub fn title_height(self) -> f64 {
        match self {
            Self::Omarchy | Self::BlackOrange | Self::Neumorphic | Self::Molded | Self::Glossy | Self::Milled => 0.0,
            Self::Macos => 32.0,
            Self::Windows => 34.0,
            Self::Windows2000 => 20.0,
            Self::NextStep => 22.0,
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
                include_str!("../../themes/omarchy/theme.splash"),
                include_str!("../../themes/omarchy/widgets.splash"),
            ),
            DesktopStyle::BlackOrange => (
                include_str!("../../themes/black-orange/theme.splash"),
                include_str!("../../themes/black-orange/widgets.splash"),
            ),
            DesktopStyle::Neumorphic => (
                include_str!("../../themes/neumorphic/theme.splash"),
                include_str!("../../themes/neumorphic/widgets.splash"),
            ),
            DesktopStyle::Molded => (
                include_str!("../../themes/molded/theme.splash"),
                include_str!("../../themes/molded/widgets.splash"),
            ),
            DesktopStyle::Glossy => (
                include_str!("../../themes/glossy/theme.splash"),
                include_str!("../../themes/glossy/widgets.splash"),
            ),
            DesktopStyle::Milled => (
                include_str!("../../themes/milled/theme.splash"),
                include_str!("../../themes/milled/widgets.splash"),
            ),
            DesktopStyle::Macos if dark => (
                include_str!("../../themes/macos-dark/theme.splash"),
                include_str!("../../themes/macos-dark/widgets.splash"),
            ),
            DesktopStyle::Macos => (
                include_str!("../../themes/macos/theme.splash"),
                include_str!("../../themes/macos/widgets.splash"),
            ),
            DesktopStyle::Windows if dark => (
                include_str!("../../themes/windows-dark/theme.splash"),
                include_str!("../../themes/windows-dark/widgets.splash"),
            ),
            DesktopStyle::Windows => (
                include_str!("../../themes/windows/theme.splash"),
                include_str!("../../themes/windows/widgets.splash"),
            ),
            DesktopStyle::Windows2000 => (
                include_str!("../../themes/windows-2000/theme.splash"),
                include_str!("../../themes/windows-2000/widgets.splash"),
            ),
            DesktopStyle::NextStep => (
                include_str!("../../themes/nextstep/theme.splash"),
                include_str!("../../themes/nextstep/widgets.splash"),
            ),
            DesktopStyle::Ios if dark => (
                include_str!("../../themes/ios-dark/theme.splash"),
                include_str!("../../themes/ios-dark/widgets.splash"),
            ),
            DesktopStyle::Ios => (
                include_str!("../../themes/ios/theme.splash"),
                include_str!("../../themes/ios/widgets.splash"),
            ),
            DesktopStyle::Android if dark => (
                include_str!("../../themes/android-dark/theme.splash"),
                include_str!("../../themes/android-dark/widgets.splash"),
            ),
            DesktopStyle::Android => (
                include_str!("../../themes/android/theme.splash"),
                include_str!("../../themes/android/widgets.splash"),
            ),
        };
        let read = |file: &str, bundled: &str| {
            // Source checkouts hotload on every selection; installed/wasm builds
            // carry the identical embedded stylesheet, with no external dependency.
            #[cfg(not(target_arch = "wasm32"))]
            if let Ok(text) = std::fs::read_to_string(
                std::path::Path::new(crate::widgets_dir())
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
/// Take the sheet off again, so the next evaluation runs under the plain
/// theme. `install` had no way back: an app that lets somebody try a sheet
/// could put one on and never return to what it started with. A sheet named
/// by `MAKEPAD_WIDGET_STYLE` comes back on the next read, as it would have
/// arrived in the first place.
pub fn uninstall(vm: &mut ScriptVm) {
    let key = vm.bx.heap.heap_key();
    vm.cx_mut().global::<Styles>().heaps.remove(&key);
}
pub fn current(vm: &mut ScriptVm) -> Option<StyleSheet> {
    let key = vm.bx.heap.heap_key();
    if let Some(sheet) = vm.cx_mut().global::<Styles>().heaps.get(&key).cloned() {
        return Some(sheet);
    }
    // Opt-in only. Picking a sheet from OsType restyled every app that had
    // never asked for one, and an app that calls `theme_mod` + `widgets_mod`
    // without `script_mod` got the theme half of it and not the widget half.
    let name = std::env::var("MAKEPAD_WIDGET_STYLE").ok()?;
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
        cargo_manifest_path: crate::widgets_dir().into(),
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
        // The library's own roles are younger than the sheets and no sheet
        // names them, so they are brought into line with what this one set.
        let roles = {
            let theme = vm.module(id!(theme));
            let mut read = |key: &str| vm.bx.heap.value(theme, LiveId::from_str(key).into(), NoTrap).as_color();
            crate::theme_tokens::sheet_roles_script(&sheet.theme, &mut read)
        };
        evaluate(vm, &sheet, "roles", roles);
        if DesktopStyle::parse(&sheet.name).is_some_and(|style| style.mobile()) {
            crate::font_policy::append_style_fallbacks(vm);
        }
    }
}
pub fn apply_widgets(vm: &mut ScriptVm) {
    if let Some(sheet) = current(vm) {
        let code = without_unregistered_widgets(vm, &sheet.widgets);
        evaluate(vm, &sheet, "widgets", code);
    }
}

/// The sheet's widget rules less every line that names a widget this app did
/// not register. A sheet styles the whole library and the widget families are
/// optional: an app built without the dates family has no `DateField`, and a
/// line reaching for one would only raise an error at every install. So a
/// sheet names a family's widget on a line of its own, and that line is
/// blanked rather than removed, so the evaluator's line numbers still point
/// into the file.
fn without_unregistered_widgets(vm: &mut ScriptVm, code: &str) -> String {
    let widgets = vm.module(id!(widgets));
    let registered = |vm: &mut ScriptVm, name: &str| {
        let value = vm.bx.heap.value(widgets, LiveId::from_str(name).into(), NoTrap);
        !(value.is_nil() || value.is_err())
    };
    let mut out: Vec<&str> = Vec::new();
    for line in code.split('\n') {
        let missing = line.match_indices("mod.widgets.").any(|(at, prefix)| {
            let name: String = line[at + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            !name.is_empty() && !registered(vm, &name)
        });
        out.push(if missing { "" } else { line });
    }
    out.join("\n")
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

    /// The fields a person types or picks a value in that are laid out by a
    /// TURTLE: they are Fit, so the vertical padding IS their height, and two
    /// of them padded differently part company again as soon as the type
    /// grows. `TextInput` heads the list because every sheet already reshaped
    /// it, and it is the one the rest of the row has to match.
    ///
    /// Left out, with reasons, so the next reader does not take the gaps for
    /// oversights: `WellInput` is the inside of a field, not a field, and is
    /// the one thing that must impose no height at all; `Select` wears a
    /// `Button` for a face and follows the button rule; `TreeSelect` names its
    /// corner `radius` and packs chips into whatever box its own Rust is
    /// handed.
    const FIELD_FIT: &[&str] = &[
        "TextInput",
        "ComboBox",
        "DropDown",
        "DropDown2",
        "FieldWell",
        "TagField",
    ];
    /// The fields that state a FRAME and lay their own parts out inside it: a
    /// number with a stepper, a number you drag, a date, a time, and the two
    /// pickers that wrap a date. The row's height is all these can take from a
    /// sheet. Each measures its parts against the turtle's whole rect and
    /// draws them from the content origin, so a vertical padding displaces the
    /// line instead of holding it off the box -- padded, the number sat on the
    /// bottom edge of its box with its two arrows split around it. They must
    /// therefore carry NO vertical padding, and that is asserted below rather
    /// than skipped, because it is the thing the next sheet would get wrong.
    const FIELD_FRAME: &[&str] = &[
        "NumberField",
        "ValueInput",
        "DateField",
        "TimeField",
        "DatePicker",
        "DateRangePicker",
    ];

    /// A field's box metrics as the sheet leaves them: the minimum height,
    /// and the padding above and below the line.
    fn field_metrics(vm: &mut ScriptVm, name: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
        let widgets = vm.module(id!(widgets));
        let widget = vm
            .bx
            .heap
            .value(widgets, LiveId::from_str(name).into(), NoTrap)
            .as_object()
            .unwrap_or_else(|| panic!("the library has no widget named {name}"));
        let min = vm.bx.heap.value(widget, id!(min_height).into(), NoTrap).as_f64();
        let pad = vm.bx.heap.value(widget, id!(padding).into(), NoTrap).as_object();
        let side = |vm: &mut ScriptVm, key: LiveId| {
            pad.and_then(|p| vm.bx.heap.value(p, key.into(), NoTrap).as_f64())
        };
        (min, side(vm, id!(top)), side(vm, id!(bottom)))
    }

    /// A row of fields is one height, under every sheet the library ships.
    ///
    /// The sheets used to reshape the text box alone: with the android sheet
    /// on, the catalogue's search box became a 48 point pill and the number
    /// field beside it stayed 24 tall, the drop down after it 26. Each sheet
    /// now states its field metrics once and hands them to the whole family,
    /// and this is what holds that: a field added to the library, or a sheet
    /// added to the folder, cannot quietly stand at a height of its own.
    #[test]
    fn every_field_stands_at_the_height_its_sheet_gives_the_text_box() {
        for (style, dark) in DesktopStyle::ALL
            .into_iter()
            .flat_map(|style| if style.supports_dark() { vec![(style, false), (style, true)] } else { vec![(style, false)] })
        {
            // A sheet of its own per appearance: an assignment a sheet makes
            // stays made, so sheets read one after another on one VM would
            // measure the last one that named a number, not this one.
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.init_cx_os();
            cx.with_vm(|vm| {
                crate::script_mod(vm);
                install(vm, StyleSheet::load_with_appearance(style, dark));
                vm.bx.captured_errors = Some(Vec::new());
                vm.with_reload(crate::script_mod);
                let sheet = if dark { format!("{}-dark", style.id()) } else { style.id().to_string() };
                assert!(vm.take_errors().is_empty(), "{sheet} does not evaluate");
                let row = field_metrics(vm, "TextInput");
                assert!(row.0.is_some(), "{sheet} states no field height for the row to stand at");
                for name in FIELD_FIT {
                    assert_eq!(
                        field_metrics(vm, name),
                        row,
                        "{sheet}: {name} does not stand in the row its TextInput sets"
                    );
                }
                // The date and time fields are the extras family's, and are
                // held to the row where that family is registered.
                let widgets = vm.module(id!(widgets));
                for name in FIELD_FRAME {
                    let known = vm.bx.heap.value(widgets, LiveId::from_str(name).into(), NoTrap).as_object();
                    if known.is_none() {
                        continue;
                    }
                    let field = field_metrics(vm, name);
                    assert_eq!(field.0, row.0, "{sheet}: {name} does not stand at the height of the row");
                    assert!(
                        field.1.unwrap_or(0.0) == 0.0 && field.2.unwrap_or(0.0) == 0.0,
                        "{sheet}: {name} lays its own parts out, so a vertical padding on it moves the line off the box"
                    );
                }
                // The chrome-less input inside a well inherits TextInput, and
                // a minimum as tall as the whole field, applied inside a well
                // already that tall, pushes the line out through the bottom.
                assert_eq!(
                    field_metrics(vm, "WellInput").0,
                    Some(0.0),
                    "{sheet}: the input inside a well must impose no height"
                );
            });
        }
    }

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
                    // The mobile policy's fallback chain (font_policy.rs) ends
                    // in the emoji face; it must survive every appearance.
                    assert!(members.contains(&"noto_color_emoji"),"{members:?}");
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
                        DesktopStyle::BlackOrange => 2.5,
                        DesktopStyle::Neumorphic => 8.0,
                        DesktopStyle::Molded => 5.0,
                        DesktopStyle::Glossy => 6.0,
                        DesktopStyle::Milled => 3.0,
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
