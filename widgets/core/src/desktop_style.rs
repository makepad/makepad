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
    /// Turned and brushed aluminium on a pale housing, chrome where a
    /// control is held.
    Aluminium,
    /// Frosted glass cards over a dark ground.
    Frosted,
    /// Clear glass on a light ground: thin bright rims, the ground seen
    /// through every face.
    Liquid,
    /// Pale lit faces on a dark ground, lit from underneath.
    Luminous,
    /// Minimal instrument hardware: a pale case, hairlines, few colours.
    FieldKit,
    /// A text interface: one face, one ink, drawn in character cells.
    Terminal,
    /// A segment display on a pale glass, unlit segments still faintly there.
    Lcd,
    /// Tubes of light on a near-black ground.
    Neon,
    /// Line frames of a head-up display over a dark ground.
    Hud,
    /// Candy-red lacquer, deep and glossy, with chrome knobs.
    Lacquer,
    /// Worn safety-yellow painted steel, black rubber and screws.
    Safety,
    /// An olive-drab field radio: stencilled type, amber and red lamps.
    FieldRadio,
    /// Steampunk brass: polished fittings, gears and pipes, parchment text.
    Brass,
    /// Futuristic moulded plastic: smooth dark shells with light traced
    /// through them.
    FuturePlastic,
    /// Futuristic brushed metal with lit circuit traces cut into it.
    FutureMetal,
    /// Glossy anthracite: dark blue-grey metal, raised caps, blue lamps.
    Anthracite,
    /// Cast concrete: grey, porous, controls stamped into it.
    Concrete,
    /// Pixel art: a pale green dot-matrix glass, controls framed in square
    /// pixels.
    Pixel,
    /// White porcelain: white on white, form drawn by light and soft
    /// shadow alone.
    Porcelain,
}

impl DesktopStyle {
    /// How many styles there are, and so how many weights a table indexed by
    /// discriminant needs: every variant is in `ALL`.
    pub const COUNT: usize = 31;
    pub const ALL: [Self; Self::COUNT] = [
        Self::Omarchy, Self::BlackOrange, Self::Neumorphic, Self::Molded, Self::Glossy, Self::Milled,
        Self::Aluminium, Self::Frosted, Self::Liquid, Self::Luminous, Self::FieldKit,
        Self::Terminal, Self::Lcd, Self::Neon, Self::Hud,
        Self::Lacquer, Self::Safety, Self::FieldRadio, Self::Brass, Self::FuturePlastic,
        Self::FutureMetal, Self::Anthracite, Self::Concrete, Self::Pixel, Self::Porcelain,
        Self::Macos, Self::Windows, Self::Windows2000, Self::NextStep, Self::Ios, Self::Android,
    ];
    /// The styles laid out as tiles rather than as floating windows, with no
    /// shelf and no title bar of their own: the tilers and every style built
    /// on the library's own surfaces rather than on somebody's desktop.
    const TILING: [Self; 25] = [
        Self::Omarchy, Self::BlackOrange, Self::Neumorphic, Self::Molded, Self::Glossy, Self::Milled,
        Self::Aluminium, Self::Frosted, Self::Liquid, Self::Luminous, Self::FieldKit,
        Self::Terminal, Self::Lcd, Self::Neon, Self::Hud,
        Self::Lacquer, Self::Safety, Self::FieldRadio, Self::Brass, Self::FuturePlastic,
        Self::FutureMetal, Self::Anthracite, Self::Concrete, Self::Pixel, Self::Porcelain,
    ];
    fn tiling(self) -> bool {
        Self::TILING.contains(&self)
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::Omarchy => "omarchy",
            Self::BlackOrange => "black-orange",
            Self::Neumorphic => "neumorphic",
            Self::Molded => "molded",
            Self::Glossy => "glossy",
            Self::Milled => "milled",
            Self::Aluminium => "aluminium",
            Self::Frosted => "frosted",
            Self::Liquid => "liquid",
            Self::Luminous => "luminous",
            Self::FieldKit => "field-kit",
            Self::Terminal => "terminal",
            Self::Lcd => "lcd",
            Self::Neon => "neon",
            Self::Hud => "hud",
            Self::Lacquer => "lacquer",
            Self::Safety => "safety",
            Self::FieldRadio => "field-radio",
            Self::Brass => "brass",
            Self::FuturePlastic => "future-plastic",
            Self::FutureMetal => "future-metal",
            Self::Anthracite => "anthracite",
            Self::Concrete => "concrete",
            Self::Pixel => "pixel",
            Self::Porcelain => "porcelain",
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
            Self::Aluminium => "Aluminium",
            Self::Frosted => "Frosted glass",
            Self::Liquid => "Liquid glass",
            Self::Luminous => "Luminous",
            Self::FieldKit => "Field kit",
            Self::Terminal => "Terminal",
            Self::Lcd => "Segment display",
            Self::Neon => "Neon",
            Self::Hud => "Head-up display",
            Self::Lacquer => "Lacquer red",
            Self::Safety => "Safety yellow",
            Self::FieldRadio => "Field radio",
            Self::Brass => "Steampunk brass",
            Self::FuturePlastic => "Futuristic plastic",
            Self::FutureMetal => "Futuristic metal",
            Self::Anthracite => "Glossy anthracite",
            Self::Concrete => "Concrete",
            Self::Pixel => "Pixel art",
            Self::Porcelain => "Porcelain white",
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
    /// Neumorphic's dark appearance is a soft dark ground of the same
    /// moulding, so it is that style's other appearance, `neumorphic-dark`,
    /// and not a style of its own: `parse` reads a `-dark` suffix as the
    /// appearance, which a style named so could never get past.
    pub fn supports_dark(self) -> bool { matches!(self, Self::Neumorphic | Self::Macos | Self::Windows | Self::Ios | Self::Android) }
    pub fn mobile(self) -> bool { matches!(self, Self::Ios | Self::Android) }
    /// Which set of app artwork this style draws, as an index into the icon
    /// table. A style is free to borrow another's drawings rather than have
    /// every icon redrawn for it -- the table is one entry per SET, not one
    /// per style, so the enum's own order must not be read as an index into
    /// it.
    pub fn icon_set(self) -> usize {
        match self {
            Self::Omarchy | Self::BlackOrange | Self::Neumorphic | Self::Glossy | Self::Milled => 0,
            Self::Aluminium | Self::Frosted | Self::Liquid | Self::Luminous | Self::FieldKit => 0,
            Self::Terminal | Self::Lcd | Self::Neon | Self::Hud => 0,
            Self::Lacquer | Self::Safety | Self::FieldRadio | Self::Brass | Self::FuturePlastic => 0,
            Self::FutureMetal | Self::Anthracite | Self::Concrete | Self::Pixel | Self::Porcelain => 0,
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
        !self.tiling() && !self.mobile()
    }
    pub fn shelf_height(self) -> f64 {
        match self {
            Self::Omarchy | Self::BlackOrange | Self::Neumorphic | Self::Molded | Self::Glossy | Self::Milled => 0.0,
            Self::Aluminium | Self::Frosted | Self::Liquid | Self::Luminous | Self::FieldKit => 0.0,
            Self::Terminal | Self::Lcd | Self::Neon | Self::Hud => 0.0,
            Self::Lacquer | Self::Safety | Self::FieldRadio | Self::Brass | Self::FuturePlastic => 0.0,
            Self::FutureMetal | Self::Anthracite | Self::Concrete | Self::Pixel | Self::Porcelain => 0.0,
            Self::Macos => 86.0,
            Self::Windows => 54.0,
            Self::Windows2000 => 34.0,
            Self::NextStep | Self::Ios | Self::Android => 0.0,
        }
    }
    pub fn title_height(self) -> f64 {
        match self {
            Self::Omarchy | Self::BlackOrange | Self::Neumorphic | Self::Molded | Self::Glossy | Self::Milled => 0.0,
            Self::Aluminium | Self::Frosted | Self::Liquid | Self::Luminous | Self::FieldKit => 0.0,
            Self::Terminal | Self::Lcd | Self::Neon | Self::Hud => 0.0,
            Self::Lacquer | Self::Safety | Self::FieldRadio | Self::Brass | Self::FuturePlastic => 0.0,
            Self::FutureMetal | Self::Anthracite | Self::Concrete | Self::Pixel | Self::Porcelain => 0.0,
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
            (DesktopStyle::Neumorphic, true) => "neumorphic-dark",
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
            DesktopStyle::Neumorphic if dark => (
                include_str!("../../themes/neumorphic-dark/theme.splash"),
                include_str!("../../themes/neumorphic-dark/widgets.splash"),
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
            DesktopStyle::Aluminium => (
                include_str!("../../themes/aluminium/theme.splash"),
                include_str!("../../themes/aluminium/widgets.splash"),
            ),
            DesktopStyle::Frosted => (
                include_str!("../../themes/frosted/theme.splash"),
                include_str!("../../themes/frosted/widgets.splash"),
            ),
            DesktopStyle::Liquid => (
                include_str!("../../themes/liquid/theme.splash"),
                include_str!("../../themes/liquid/widgets.splash"),
            ),
            DesktopStyle::Luminous => (
                include_str!("../../themes/luminous/theme.splash"),
                include_str!("../../themes/luminous/widgets.splash"),
            ),
            DesktopStyle::FieldKit => (
                include_str!("../../themes/field-kit/theme.splash"),
                include_str!("../../themes/field-kit/widgets.splash"),
            ),
            DesktopStyle::Terminal => (
                include_str!("../../themes/terminal/theme.splash"),
                include_str!("../../themes/terminal/widgets.splash"),
            ),
            DesktopStyle::Lcd => (
                include_str!("../../themes/lcd/theme.splash"),
                include_str!("../../themes/lcd/widgets.splash"),
            ),
            DesktopStyle::Neon => (
                include_str!("../../themes/neon/theme.splash"),
                include_str!("../../themes/neon/widgets.splash"),
            ),
            DesktopStyle::Hud => (
                include_str!("../../themes/hud/theme.splash"),
                include_str!("../../themes/hud/widgets.splash"),
            ),
            DesktopStyle::Lacquer => (
                include_str!("../../themes/lacquer/theme.splash"),
                include_str!("../../themes/lacquer/widgets.splash"),
            ),
            DesktopStyle::Safety => (
                include_str!("../../themes/safety/theme.splash"),
                include_str!("../../themes/safety/widgets.splash"),
            ),
            DesktopStyle::FieldRadio => (
                include_str!("../../themes/field-radio/theme.splash"),
                include_str!("../../themes/field-radio/widgets.splash"),
            ),
            DesktopStyle::Brass => (
                include_str!("../../themes/brass/theme.splash"),
                include_str!("../../themes/brass/widgets.splash"),
            ),
            DesktopStyle::FuturePlastic => (
                include_str!("../../themes/future-plastic/theme.splash"),
                include_str!("../../themes/future-plastic/widgets.splash"),
            ),
            DesktopStyle::FutureMetal => (
                include_str!("../../themes/future-metal/theme.splash"),
                include_str!("../../themes/future-metal/widgets.splash"),
            ),
            DesktopStyle::Anthracite => (
                include_str!("../../themes/anthracite/theme.splash"),
                include_str!("../../themes/anthracite/widgets.splash"),
            ),
            DesktopStyle::Concrete => (
                include_str!("../../themes/concrete/theme.splash"),
                include_str!("../../themes/concrete/widgets.splash"),
            ),
            DesktopStyle::Pixel => (
                include_str!("../../themes/pixel/theme.splash"),
                include_str!("../../themes/pixel/widgets.splash"),
            ),
            DesktopStyle::Porcelain => (
                include_str!("../../themes/porcelain/theme.splash"),
                include_str!("../../themes/porcelain/widgets.splash"),
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
/// Keep the library as it stands before any sheet, for the chrome that must
/// look and measure the same under every sheet: `mod.stock_widgets`, the
/// stock templates, and `mod.prelude.stock_internal`, the prelude with the
/// stock theme under `theme`.
///
/// A sheet reaches a template two ways. Its token half runs before any
/// template registers, so every token a template bakes -- a spacing rung, a
/// font, a corner, an animation time -- is the sheet's; its widget half runs
/// after, and writes straight onto the templates, so every template derived
/// from one it wrote to takes what it wrote -- a face, a margin, an animator
/// state -- wherever it does not say otherwise. The developer panel, the fab
/// controls and a host's own tool panels are built from those templates. So
/// while a sheet is installed, the module run registers the library twice:
/// once without the sheet, kept here, and once with it, which is what
/// `mod.widgets` holds and what the sheet's widget half writes into. Chrome
/// takes its templates and its theme from the kept one, after whatever else
/// it uses:
///
/// ```text
/// use mod.prelude.stock_internal.*
/// use mod.stock_widgets.*
/// ```
///
/// With no sheet installed there is one registration, and the kept library
/// is `mod.widgets` itself.
pub(crate) fn keep_stock(vm: &mut ScriptVm) {
    script_eval!(vm, {
        mod.stock_widgets = mod.widgets
    });
    script_eval!(vm, {
        mod.prelude.stock_internal = {
            ..mod.prelude.widgets_header,
            theme: mod.theme,
        }
    });
}

/// What an object holds or inherits, as (name, value): its own keys and its
/// prototypes', resolved on the object, and its children -- its own, or,
/// where it was made without a copy of them, the nearest prototype's.
fn resolved_entries(heap: &ScriptHeap, obj: ScriptObject) -> Vec<(String, ScriptValue)> {
    let mut keys: Vec<ScriptValue> = Vec::new();
    let mut at = Some(obj);
    while let Some(o) = at {
        for (key, _) in heap.map_ref(o).iter() {
            if !keys.contains(key) {
                keys.push(*key);
            }
        }
        at = heap.proto(o).as_object();
    }
    let name = |key: ScriptValue| key.as_id().map(|id| id.to_string()).unwrap_or_else(|| "_".into());
    let mut out: Vec<(String, ScriptValue)> = keys.into_iter().map(|key| (name(key), heap.value(obj, key, NoTrap))).collect();
    let mut at = Some(obj);
    while let Some(o) = at {
        let vec = heap.vec_ref(o);
        if !vec.is_empty() {
            out.extend(vec.iter().map(|entry| (name(entry.key), entry.value)));
            break;
        }
        at = heap.proto(o).as_object();
    }
    out
}

/// A name a sheet can write after a dot.
fn writable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_uppercase() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with("__")
}

/// A sheet that sets everything a sheet may, read off the stock library: a
/// value of its own for every token the stock theme holds, and for every
/// number, colour, switch, inset, text style and face (`pixel` and `vertex`)
/// of every template the stock library registered, down through each draw
/// object, animator state and nested part the template holds of its own.
/// What a host's tests install to prove that its chrome resolves the same
/// under any sheet; see [`resolution`]. Build it with no sheet installed, so
/// that what it reads is what a sheet would find. `spare` names templates the
/// sheet leaves alone: a host's own controls, registered beside the stock
/// ones, which no sheet has any business writing to by name.
#[doc(hidden)]
pub fn everything_sheet(vm: &mut ScriptVm, spare: &dyn Fn(&str) -> bool) -> StyleSheet {
    let theme = script_eval!(vm, {mod.prelude.stock_internal.theme});
    let inset = script_eval!(vm, {mod.turtle.Inset});
    let text_style = script_eval!(vm, {mod.text.TextStyle});
    let widgets = vm.module(id!(stock_widgets));
    let font = "TextStyle{font_family: FontFamily{latin := FontMember{res: crate_resource(\"self:resources/Inter.ttf\") weight: 400.0 asc: 0.0 desc: 0.0}} font_size: 19.0 line_spacing: 1.7}";
    let heap = &vm.bx.heap;
    let is = |value: ScriptValue, proto: ScriptValue| {
        let mut at = value.as_object();
        while let Some(o) = at {
            if ScriptValue::from(o) == proto {
                return true;
            }
            at = heap.proto(o).as_object();
        }
        false
    };
    // A leaf's new value as a sheet would write it, or `None` for what is
    // not a leaf a sheet sets (text, names, enums, objects to walk into).
    let leaf = |name: &str, value: ScriptValue| -> Option<String> {
        if let Some(o) = value.as_object() {
            if heap.is_fn(o) {
                return match name {
                    "pixel" => Some("fn() { return #ff00ffff }".into()),
                    "vertex" => Some("fn() { self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size) }".into()),
                    _ => None,
                };
            }
            if is(value, inset) {
                return Some("mod.turtle.Inset{top: 9.5 right: 9.5 bottom: 9.5 left: 9.5}".into());
            }
            if is(value, text_style) {
                return Some(font.into());
            }
            // `uniform(x)` and `instance(x)`: a number under a wrapper.
            if let Some(number) = heap.proto(o).as_f64() {
                return Some(format!("{:?}", number + 1.25));
            }
            return None;
        }
        if let Some(color) = value.as_color() {
            return Some(format!("#{:08x}", color ^ 0x5a3c9600));
        }
        if let Some(b) = value.as_bool() {
            return Some(format!("{}", !b));
        }
        if value.is_f64() {
            let number = value.as_f64()?;
            return number.is_finite().then(|| format!("{:?}", number + 1.25));
        }
        None
    };

    let mut tokens = String::from("mod.theme = mod.themes.dark\nuse mod.res.*\nuse mod.text.*\n");
    if let Some(theme) = theme.as_object() {
        let mut entries = resolved_entries(heap, theme);
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, value) in entries {
            if !writable_name(&name) {
                continue;
            }
            if let Some(text) = leaf(&name, value) {
                tokens.push_str(&format!("mod.theme.{name} = {text}\n"));
            }
        }
    }
    tokens.push_str("true\n");

    let mut writes = String::from("use mod.prelude.widgets_internal.*\n");
    let mut names: Vec<String> = heap.map_ref(widgets).iter().filter_map(|(key, _)| key.as_id().map(|id| id.to_string())).collect();
    names.sort();
    let mut seen: std::collections::HashSet<ScriptObject> = std::collections::HashSet::new();
    // Every object once, through the first path that reaches it: a write
    // lands on the object the path resolves to, whichever template names it.
    // Into the parts a template holds of its own, and not into those it
    // inherits: those are another template's, and are written through that
    // one's name.
    // A part is the template's own when it was made in the same source as
    // the template: a value the template only names -- a layout constant, a
    // token's inset, another template -- is somebody else's, shared with
    // everything else that names it, and a sheet replaces it rather than
    // writing into it.
    let mut stack: Vec<(ScriptObject, String, usize, u16)> = Vec::new();
    for name in names.iter().rev() {
        let value = heap.value(widgets, LiveId::from_str(name).into(), NoTrap);
        if let Some(o) = value.as_object() {
            if writable_name(name) && !spare(name) && !heap.is_fn(o) {
                stack.push((o, format!("mod.widgets.{name}"), 0, heap.object_data(o).made_at.body));
            }
        }
    }
    while let Some((obj, path, depth, body)) = stack.pop() {
        if !seen.insert(obj) || heap.object_data(obj).tag.is_immutable() {
            continue;
        }
        for (name, value) in resolved_entries(heap, obj) {
            if !writable_name(&name) {
                continue;
            }
            if let Some(text) = leaf(&name, value) {
                writes.push_str(&format!("{path}.{name} = {text}\n"));
            }
        }
        let own = heap.map_ref(obj).iter().map(|(key, value)| (*key, value.value)).chain(heap.vec_ref(obj).iter().map(|entry| (entry.key, entry.value)));
        let mut children = Vec::new();
        for (key, value) in own {
            let Some(name) = key.as_id().map(|id| id.to_string()) else { continue };
            if !writable_name(&name) || leaf(&name, value).is_some() {
                continue;
            }
            if let Some(child) = value.as_object() {
                if depth < 8 && !heap.is_fn(child) && heap.object_data(child).made_at.body == body {
                    children.push((child, format!("{path}.{name}"), depth + 1, body));
                }
            }
        }
        stack.extend(children.into_iter().rev());
    }
    writes.push_str("true\n");
    StyleSheet {
        name: "everything".into(),
        theme: tokens,
        widgets: writes,
        icons: Vec::new(),
    }
}

/// What `root` resolves to, as one line per leaf: `path = value`, walking
/// everything the object holds or inherits, its children and theirs. An
/// object met again is named by the path it was first met at. Two readings
/// of the same templates, taken in two module runs, are equal line for line
/// exactly when every value, face and nested part resolves the same; a face
/// is read as the place in the source it was written, so a sheet's face and
/// the stock one tell apart. See [`everything_sheet`].
#[doc(hidden)]
pub fn resolution(vm: &mut ScriptVm, root: ScriptValue, what: &str) -> Vec<String> {
    let heap = &vm.bx.heap;
    fn text(heap: &ScriptHeap, value: ScriptValue) -> String {
        if let Some(color) = value.as_color() {
            format!("#{color:08x}")
        } else if value.is_string_like() {
            heap.string_with(value, |_, s| format!("{s:?}")).unwrap_or_default()
        } else if let Some(pod) = value.as_pod() {
            let (ty, words) = heap.pod_data(pod);
            format!("{:?}{words:?}", ty.name)
        } else if let Some(array) = value.as_array() {
            match heap.array_storage(array) {
                makepad_script::ScriptArrayStorage::ScriptValue(items) => {
                    let items: Vec<String> = items
                        .iter()
                        .map(|item| match item.as_object() {
                            Some(o) => match heap.as_fn(o) {
                                Some(face) => format!("fn {face:?}"),
                                None => "{..}".into(),
                            },
                            None => text(heap, *item),
                        })
                        .collect();
                    format!("[{}]", items.join(", "))
                }
                makepad_script::ScriptArrayStorage::F32(items) => format!("{items:?}"),
                makepad_script::ScriptArrayStorage::U32(items) => format!("{items:?}"),
                makepad_script::ScriptArrayStorage::U16(items) => format!("{items:?}"),
                makepad_script::ScriptArrayStorage::U8(items) => format!("{items:?}"),
            }
        } else {
            format!("{value:?}")
        }
    }
    let mut out = Vec::new();
    let mut first: HashMap<ScriptObject, String> = HashMap::new();
    let mut stack: Vec<(ScriptValue, String)> = vec![(root, what.to_string())];
    while let Some((value, path)) = stack.pop() {
        if out.len() > 400_000 {
            out.push("... cut short".into());
            break;
        }
        let Some(obj) = value.as_object() else {
            out.push(format!("{path} = {}", text(heap, value)));
            continue;
        };
        if let Some(face) = heap.as_fn(obj) {
            out.push(format!("{path} = fn {face:?}"));
            continue;
        }
        if let Some(at) = first.get(&obj) {
            out.push(format!("{path} = @{at}"));
            continue;
        }
        first.insert(obj, path.clone());
        let mut entries = resolved_entries(heap, obj);
        // The geometry a draw object is handed is made for each module run,
        // and is not a thing a sheet sets.
        entries.retain(|(name, _)| name != "geom");
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        // A wrapper round a value (`uniform(x)`) is read as its value.
        if entries.is_empty() {
            let proto = heap.proto(obj);
            if proto.as_object().is_none() {
                out.push(format!("{path} = {}", text(heap, proto)));
                continue;
            }
        }
        for (name, value) in entries.into_iter().rev() {
            stack.push((value, format!("{path}.{name}")));
        }
    }
    out
}

/// The lines of two readings that differ, the first `limit` of them, for a
/// failure message.
#[doc(hidden)]
pub fn resolution_diff(want: &[String], got: &[String], limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    for (at, (w, g)) in want.iter().zip(got.iter()).enumerate() {
        if w != g {
            out.push(format!("line {at}: want {w}\n            got  {g}"));
            if out.len() >= limit {
                break;
            }
        }
    }
    if want.len() != got.len() && out.len() < limit {
        out.push(format!("{} lines wanted, {} read", want.len(), got.len()));
    }
    out
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
    /// Every face a sheet gives the app keeps the app's fallbacks behind it.
    ///
    /// A family is the face a sheet picks FIRST and then the faces the font
    /// policy puts after it -- the scripts the first face has no glyphs for,
    /// the emoji -- so that text typed into a field is spelled whatever it
    /// holds. A sheet that writes `FontFamily{latin := ..}` builds a family of
    /// one face, and every one of those is lost; and one that adds a member
    /// to the base's family (`font_family{latin := ..}`) puts it LAST, where
    /// it is only a fallback. The way to change the face is to replace the
    /// family's first member by its name, the family otherwise the base's:
    /// `mod.theme.font_regular.font_family{ibm_plex_text := FontMember{..}}`.
    ///
    /// Read under every sheet the library ships: the theme's own faces, and
    /// every text style a sheet's widget half writes onto a template. Each
    /// must end in the fallbacks of one of the stock faces that has any --
    /// the regular's or the bold's, whichever it was built on. A template
    /// may also wear a stock face as it is (the code face, which is one face
    /// in the stock theme too); the theme's own faces may not, since
    /// everything the app writes is written in them.
    #[test]
    fn every_sheet_keeps_the_fallbacks_behind_its_own_face() {
        const FACES: &[&str] = &["font_regular", "font_label", "font_bold", "font_italic", "font_bold_italic", "font_code"];
        let members = |vm: &mut ScriptVm, value: ScriptValue| -> Vec<String> {
            let text = TextStyle::script_from_value(vm, value);
            text.font_family.member_ids().map(|id| id.to_string()).collect()
        };
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let theme = vm.module(id!(theme));
            // The code face is one face in the stock theme too, so a sheet
            // has nothing of it to keep; the others are read.
            let mut stock: Vec<Vec<String>> = Vec::new();
            let mut whole: Vec<Vec<String>> = Vec::new();
            let mut faces: Vec<&str> = Vec::new();
            for face in FACES {
                let value = vm.bx.heap.value(theme, LiveId::from_str(face).into(), NoTrap);
                let have = members(vm, value);
                if have.len() > 1 {
                    stock.push(have[1..].to_vec());
                    faces.push(face);
                }
                whole.push(have);
            }
            assert!(stock.len() >= 2, "the stock faces have no fallbacks to keep: {stock:?}");
            let mut checked = 0;
            for style in DesktopStyle::ALL {
                for dark in [false, true] {
                    if dark && !style.supports_dark() {
                        continue;
                    }
                    let sheet = StyleSheet::load_with_appearance(style, dark);
                    install(vm, sheet.clone());
                    vm.with_reload(crate::script_mod);
                    assert!(vm.take_errors().is_empty(), "{} does not evaluate", sheet.name);
                    let mut sites: Vec<(String, ScriptValue)> = Vec::new();
                    let theme = vm.module(id!(theme));
                    for face in &faces {
                        let value = vm.bx.heap.value(theme, LiveId::from_str(face).into(), NoTrap);
                        sites.push((format!("theme.{face}"), value));
                    }
                    let widgets = vm.module(id!(widgets));
                    for line in sheet.widgets.lines() {
                        let Some(path) = line
                            .trim()
                            .strip_prefix("mod.widgets.")
                            .and_then(|rest| rest.split_once(" = "))
                            .map(|(path, _)| path.trim())
                            .filter(|path| path.ends_with(".text_style"))
                        else {
                            continue;
                        };
                        let ids: Vec<LiveId> = path.split('.').map(LiveId::from_str).collect();
                        let value = vm.bx.heap.value_path(widgets, &ids, NoTrap);
                        sites.push((path.to_string(), value));
                    }
                    for (site, value) in sites {
                        assert!(value.as_object().is_some(), "{}: `{site}` did not resolve", sheet.name);
                        let have = members(vm, value);
                        let stock_face = !site.starts_with("theme.") && whole.contains(&have);
                        assert!(
                            stock_face || stock.iter().any(|fallbacks| fallbacks.iter().all(|id| have[1..].contains(id))),
                            "{}: `{site}` lost the fallbacks: it has {have:?}, and every stock face ends in one of {stock:?}",
                            sheet.name
                        );
                        checked += 1;
                    }
                }
            }
            assert!(checked > 150, "only {checked} faces were read");
            uninstall(vm);
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
                        DesktopStyle::Glossy => 1.0,
                        DesktopStyle::Milled | DesktopStyle::Aluminium | DesktopStyle::Frosted | DesktopStyle::Neon => 2.0,
                        DesktopStyle::Liquid | DesktopStyle::Luminous => 3.0,
                        DesktopStyle::FieldKit => 2.5,
                        DesktopStyle::Lcd => 1.5,
                        DesktopStyle::Lacquer | DesktopStyle::Brass | DesktopStyle::Anthracite => 3.0,
                        DesktopStyle::Safety | DesktopStyle::FieldRadio | DesktopStyle::FutureMetal => 2.0,
                        DesktopStyle::FuturePlastic => 6.0,
                        DesktopStyle::Concrete => 1.0,
                        DesktopStyle::Porcelain => 4.0,
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
    /// A table indexed by discriminant is `COUNT` long, and every variant
    /// is in `ALL` exactly once, so no style falls off the end of one.
    #[test]
    fn every_style_is_listed_once_and_fits_the_count() {
        for (at, style) in DesktopStyle::ALL.into_iter().enumerate() {
            assert!((style as usize) < DesktopStyle::COUNT, "{style:?}");
            assert_eq!(DesktopStyle::ALL.iter().position(|s| *s == style), Some(at), "{style:?} twice");
            assert_eq!(DesktopStyle::parse(style.id()), Some(style));
        }
        // The styles built on the library's own surfaces tile, as the
        // tilers do; only the desktops modelled on somebody else's float.
        for style in DesktopStyle::TILING {
            assert!(!style.floating(), "{style:?} floats");
            assert_eq!(style.shelf_height(), 0.0);
            assert_eq!(style.title_height(), 0.0);
        }
    }

    /// A sheet lays a window ground with two lines, and a window under a
    /// sheet that writes neither keeps its ground off, so it only clears as
    /// it always did. Read off a window made from the template the way an
    /// app makes one, since that is where the sheet's writes have to land.
    #[test]
    fn a_sheet_lays_the_window_ground_and_only_a_sheet_that_asks_for_one() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let shows = |vm: &mut ScriptVm| {
                let window = script_eval!(vm, {mod.widgets.Window{}});
                let obj = window.as_object().expect("a window object");
                vm.bx.heap.value(obj, id!(show_bg).into(), NoTrap).as_bool()
            };
            assert_eq!(shows(vm), Some(false), "the stock window draws no ground");
            for style in DesktopStyle::ALL {
                let sheet = StyleSheet::load(style);
                let asks = sheet.widgets.contains("mod.widgets.Window.show_bg = true");
                install(vm, sheet);
                vm.with_reload(crate::script_mod);
                assert_eq!(shows(vm), Some(asks), "{}: the window's ground", style.id());
            }
            let ground = StyleSheet {
                name: "ground".into(),
                theme: "mod.theme = mod.themes.dark\ntrue\n".into(),
                widgets: "use mod.prelude.widgets_internal.*\n\
                          mod.widgets.Window.show_bg = true\n\
                          mod.widgets.Window.draw_bg.pixel = fn() {\n\
                              let p = self.pos * self.rect_size\n\
                              let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)\n\
                              let g = Finish.grain(p / px, 0.01)\n\
                              return vec4(self.color.rgb + vec3(g, g, g), 1.0)\n\
                          }\n\
                          true\n"
                    .into(),
                icons: Vec::new(),
            };
            install(vm, ground);
            vm.bx.captured_errors = Some(Vec::new());
            vm.with_reload(crate::script_mod);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "the ground sheet does not evaluate: {errors:?}");
            assert_eq!(shows(vm), Some(true), "the sheet's ground is on");
            let source = script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.Window.draw_bg, "hlsl", false)});
            let text = vm.bx.heap.string_with(source, |_heap, text| text.to_string()).expect("source");
            assert!(!text.starts_with("ERRORS"), "the sheet's ground does not compile: {text}");
            assert!(text.contains("_grain("), "the window draws the sheet's pixel, not its own");
            // And a switch back to a sheet without a ground turns it off.
            install(vm, StyleSheet::load(DesktopStyle::Macos));
            vm.with_reload(crate::script_mod);
            assert_eq!(shows(vm), Some(false), "the ground outlived its sheet");
            uninstall(vm);
        });
    }

    /// The instruments are restyled from a sheet exactly as the stock
    /// controls are: a uniform written, a pixel function replaced, the
    /// face's own helpers and instances read. This is the handbook's worked
    /// example, held to evaluating and compiling.
    #[test]
    fn a_sheet_restyles_the_readout_and_the_lamp() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let sheet = StyleSheet {
                name: "instruments".into(),
                theme: "mod.theme = mod.themes.dark\nmod.theme.color_screen_ink = #ffb347\ntrue\n".into(),
                widgets: "use mod.prelude.widgets_internal.*\n\
                          mod.widgets.Readout.draw_bg.stroke = 0.15\n\
                          mod.widgets.Readout.draw_bg.pixel = fn() {\n\
                              let h = max(self.rect_size.y, 1.0)\n\
                              let over = (self.rect_size.x - self.span) * 0.5\n\
                              let p = (self.pos * self.rect_size - vec2(over, 0.0)) / h\n\
                              let q = vec2(p.x + (p.y - 0.5) * self.slant, p.y)\n\
                              let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5) / h\n\
                              let x0 = (self.span / h - self.glyph_width) * 0.3\n\
                              let dd = self.digit(q - vec2(x0, 0.0))\n\
                              let a = max(Finish.cover(dd.x, px), Finish.cover(dd.y, px) * self.ghost) * self.opacity\n\
                              return vec4(self.color.rgb * a, a)\n\
                          }\n\
                          mod.widgets.Lamp.draw_bg.pixel = fn() {\n\
                              let p = self.pos * self.rect_size\n\
                              let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)\n\
                              let reach = clamp(self.reach, 0.0, 1.0)\n\
                              let t = self.rect_size.y / (1.0 + 2.0 * reach)\n\
                              let c = self.rect_size * 0.5\n\
                              let hb = vec2(max((self.rect_size.x - 2.0 * t * reach) * 0.5, t * 0.5), t * 0.5)\n\
                              let d = Finish.sd_chamfer(p, c, hb, t * 0.2)\n\
                              let ink = self.intent_color()\n\
                              let lit = clamp(self.lit, 0.0, 1.0)\n\
                              let body = mix(self.color_off.rgb, ink.rgb, lit)\n\
                              let a = Finish.cover(d, px)\n\
                              let h = self.halo_at(d, t, px, self.halo * lit, reach)\n\
                              return Finish.over(self.halo_light(ink.rgb, h), vec4(body * a, a)) * self.opacity\n\
                          }\n\
                          true\n"
                    .into(),
                icons: Vec::new(),
            };
            install(vm, sheet);
            vm.bx.captured_errors = Some(Vec::new());
            vm.with_reload(crate::script_mod);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "the example sheet does not evaluate: {errors:?}");
            for (name, own, value) in [
                ("Readout", "_digit(", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.Readout.draw_bg, "hlsl", false)})),
                ("Lamp", "_sd_chamfer(", script_eval!(vm, {mod.shader.test_compile_draw_source(mod.widgets.Lamp.draw_bg, "hlsl", false)})),
            ] {
                let text = vm.bx.heap.string_with(value, |_heap, text| text.to_string()).expect("source");
                assert!(!text.starts_with("ERRORS"), "{name} does not compile under the example: {text}");
                assert!(text.contains(own), "{name} does not draw the sheet's face");
            }
            uninstall(vm);
        });
    }

    /// Under a sheet, what a sheet can change about the library as it stands
    /// without one, read the way `resolution` reads it: the stock theme and
    /// every template the stock library registered, and the same templates
    /// as `mod.widgets` holds them.
    fn stock_and_live(vm: &mut ScriptVm) -> (Vec<String>, Vec<String>, Vec<String>) {
        let theme = script_eval!(vm, {mod.prelude.stock_internal.theme});
        let stock = vm.module(id!(stock_widgets));
        let widgets = vm.module(id!(widgets));
        let theme = resolution(vm, theme, "theme");
        let stock = resolution(vm, stock.into(), "stock");
        let live = ["Button", "TextInput", "CheckBox", "DropDown", "ScrollBar", "Label"]
            .into_iter()
            .flat_map(|name| {
                let value = vm.bx.heap.value(widgets, LiveId::from_str(name).into(), NoTrap);
                resolution(vm, value, name)
            })
            .collect();
        (theme, stock, live)
    }

    /// The library as it stands without a sheet resolves the same under any
    /// sheet: under one that sets everything a sheet may -- every token, and
    /// every number, colour, switch, inset, text style and face of every
    /// template -- the stock theme and every template in the stock library
    /// read exactly as they do with no sheet installed. That is what the
    /// developer panel, the fab controls and a host's tool panels are built
    /// from (`keep_stock`), so what holds here holds for them.
    ///
    /// And the same templates as `mod.widgets` holds them do NOT read the
    /// same, which says the sheet was installed, reached the library, and
    /// the reading can tell.
    #[test]
    fn the_stock_library_resolves_the_same_under_a_sheet_that_sets_everything() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let (theme, stock, live) = stock_and_live(vm);
            assert!(stock.len() > 10_000, "only {} lines were read off the stock library", stock.len());
            let sheet = everything_sheet(vm, &|_| false);
            assert!(sheet.theme.lines().count() > 400, "the sheet sets only {} tokens", sheet.theme.lines().count());
            assert!(sheet.widgets.lines().count() > 10_000, "the sheet sets only {} leaves", sheet.widgets.lines().count());
            install(vm, sheet);
            vm.bx.captured_errors = Some(Vec::new());
            vm.with_reload(crate::script_mod);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "the sheet does not evaluate: {errors:?}");
            let (sheet_theme, sheet_stock, sheet_live) = stock_and_live(vm);
            let moved = resolution_diff(&theme, &sheet_theme, 20);
            assert!(moved.is_empty(), "the stock theme moved under the sheet:\n{}", moved.join("\n"));
            let moved = resolution_diff(&stock, &sheet_stock, 20);
            assert!(moved.is_empty(), "the stock library moved under the sheet:\n{}", moved.join("\n"));
            assert!(
                resolution_diff(&live, &sheet_live, 1).len() == 1,
                "the sheet reached nothing in `mod.widgets`, so nothing above was tested"
            );
            uninstall(vm);
            vm.with_reload(crate::script_mod);
            let (_, back, back_live) = stock_and_live(vm);
            assert!(resolution_diff(&stock, &back, 1).is_empty(), "taking the sheet off did not give the library back");
            assert!(resolution_diff(&live, &back_live, 1).is_empty(), "taking the sheet off did not give the templates back");
        });
    }

    #[test]
    fn stylesheet_wire_preserves_both_splash_phases() {
        let sheet = StyleSheet::load(DesktopStyle::Windows2000);
        assert_eq!(StyleSheet::parse(&sheet.to_json()), Some(sheet));
        assert!(StyleSheet::parse("{\"wm\":\"Adopted\"}").is_none());
    }
}
