//! Hotloadable application styles. The WM owns framebuffer transitions; this
//! module only installs Splash definitions and reapplies the existing widget tree.
//!
//! A style is a sheet in the catalogue ([`SheetEntry`]): its Splash sources,
//! and the desktop family ([`DesktopStyle`]) it lays out as. The library's own
//! desktop sheets are the catalogue's first entries; an app adds its own with
//! [`register`] at startup, before the first style load, and every list of
//! styles -- the pickers, the theme lab, the storybook -- reads the catalogue.
use crate::*;
use makepad_micro_serde::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicPtr, Ordering};

/// A desktop family: how the WM lays windows out (floating or tiled, a shelf,
/// a title bar), which artwork its app icons draw and which of the WM's
/// desktop identities it tweens to. Every sheet names the family it lays out
/// as; a sheet is not a family.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SerJson, DeJson)]
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
    /// 4 NeXTSTEP), so a new family takes the next number and `ALL` below
    /// keeps the order they are shown in.
    BlackOrange,
}

impl DesktopStyle {
    /// How many families there are, and so how many weights a table indexed
    /// by discriminant needs: every variant is in `ALL`.
    pub const COUNT: usize = 8;
    pub const ALL: [Self; Self::COUNT] = [
        Self::Omarchy, Self::BlackOrange, Self::Macos, Self::Windows, Self::Windows2000, Self::NextStep, Self::Ios, Self::Android,
    ];
    /// The families laid out as tiles rather than as floating windows, with
    /// no shelf and no title bar of their own.
    const TILING: [Self; 2] = [Self::Omarchy, Self::BlackOrange];
    fn tiling(self) -> bool {
        Self::TILING.contains(&self)
    }
    /// The family's id, which is also the id of its own sheet in the
    /// catalogue.
    pub fn id(self) -> &'static str {
        match self {
            Self::Omarchy => "omarchy",
            Self::BlackOrange => "black-orange",
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
            Self::Macos => "macOS",
            Self::Windows => "Windows",
            Self::Windows2000 => "Windows 2000",
            Self::NextStep => "NeXTSTEP",
            Self::Ios => "iOS",
            Self::Android => "Android",
        }
    }
    /// The family of this id. A sheet's name is not a family's: look a sheet
    /// up with [`find`] and read its `family`.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.id() == s)
    }
    /// Whether the family's desktop has a dark appearance, which its own
    /// sheet then offers as its `dark` variant.
    pub fn supports_dark(self) -> bool { matches!(self, Self::Macos | Self::Windows | Self::Ios | Self::Android) }
    pub fn mobile(self) -> bool { matches!(self, Self::Ios | Self::Android) }
    /// Which set of app artwork this family draws, as an index into the icon
    /// table. A family is free to borrow another's drawings rather than have
    /// every icon redrawn for it -- the table is one entry per SET, not one
    /// per family, so the enum's own order must not be read as an index into
    /// it.
    pub fn icon_set(self) -> usize {
        match self {
            Self::Omarchy | Self::BlackOrange => 0,
            Self::Macos => 1,
            Self::Windows => 2,
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
            Self::Omarchy | Self::BlackOrange => 0.0,
            Self::Macos => 86.0,
            Self::Windows => 54.0,
            Self::Windows2000 => 34.0,
            Self::NextStep | Self::Ios | Self::Android => 0.0,
        }
    }
    pub fn title_height(self) -> f64 {
        match self {
            Self::Omarchy | Self::BlackOrange => 0.0,
            Self::Macos => 32.0,
            Self::Windows => 34.0,
            Self::Windows2000 => 20.0,
            Self::NextStep => 22.0,
            Self::Ios | Self::Android => 0.0,
        }
    }
    /// The family's own sheet in the catalogue, in the appearance asked for
    /// where it has one.
    pub fn sheet(self, dark: bool) -> &'static SheetEntry {
        find(self.id())
            .unwrap_or_else(|| panic!("the catalogue has no sheet for the {} family", self.id()))
            .with_appearance(dark)
    }
}

/// One sheet in the catalogue: what it is called, the family it lays out as,
/// and its two Splash halves. Build one with [`sheet_entry!`], which embeds
/// the sources from the owning crate.
#[derive(Debug)]
pub struct SheetEntry {
    /// The id a setting, the `MAKEPAD_WIDGET_STYLE` variable and the WM name
    /// it by, and the directory under `source_dir` its files live in.
    pub id: &'static str,
    /// What a picker shows.
    pub label: &'static str,
    /// The desktop family it lays out as.
    pub family: DesktopStyle,
    /// The id of its dark appearance, itself a sheet in the catalogue.
    pub dark: Option<&'static str>,
    /// The token half, embedded.
    pub theme: &'static str,
    /// The widget half, embedded.
    pub widgets: &'static str,
    /// The directory holding `<id>/theme.splash` and `<id>/widgets.splash` in
    /// a source checkout (the owning crate's), hotloaded on every load.
    pub source_dir: &'static str,
    /// The cargo manifest directory the sheet's `crate_resource("self:...")`
    /// resolves against: the crate that owns the fonts and images it names.
    pub resources: fn() -> &'static str,
}

impl SheetEntry {
    /// This sheet in the appearance asked for: its dark variant for `dark`
    /// where it has one, its light one otherwise.
    pub fn with_appearance(&'static self, dark: bool) -> &'static SheetEntry {
        if dark {
            self.dark.and_then(find).unwrap_or(self)
        } else {
            self.light()
        }
    }
    /// The sheet whose dark variant this one is, or itself.
    pub fn light(&'static self) -> &'static SheetEntry {
        catalogue().into_iter().find(|entry| entry.dark == Some(self.id)).unwrap_or(self)
    }
    /// Whether this is another sheet's dark variant.
    pub fn is_dark_variant(&'static self) -> bool {
        !std::ptr::eq(self.light(), self)
    }
}

/// A [`SheetEntry`] for the sheet in `<dir>/<id>/`, `dir` relative to the
/// invoking crate's manifest directory: the sources are embedded from there,
/// and a source checkout hotloads them from there.
#[macro_export]
macro_rules! sheet_entry {
    (dir: $dir:literal, id: $id:literal, label: $label:literal, family: $family:expr, dark: $dark:expr, resources: $resources:expr $(,)?) => {
        $crate::desktop_style::SheetEntry {
            id: $id,
            label: $label,
            family: $family,
            dark: $dark,
            theme: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir, "/", $id, "/theme.splash")),
            widgets: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir, "/", $id, "/widgets.splash")),
            source_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir),
            resources: $resources,
        }
    };
}

/// The library's own sheets: the desktop families', each followed by its
/// dark appearance where it has one.
static DESKTOP_SHEETS: [SheetEntry; 12] = [
    sheet_entry!(dir: "../themes", id: "omarchy", label: "Omarchy", family: DesktopStyle::Omarchy, dark: None, resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "black-orange", label: "Black orange", family: DesktopStyle::BlackOrange, dark: None, resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "macos", label: "macOS", family: DesktopStyle::Macos, dark: Some("macos-dark"), resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "macos-dark", label: "macOS dark", family: DesktopStyle::Macos, dark: None, resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "windows", label: "Windows", family: DesktopStyle::Windows, dark: Some("windows-dark"), resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "windows-dark", label: "Windows dark", family: DesktopStyle::Windows, dark: None, resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "windows-2000", label: "Windows 2000", family: DesktopStyle::Windows2000, dark: None, resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "nextstep", label: "NeXTSTEP", family: DesktopStyle::NextStep, dark: None, resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "ios", label: "iOS", family: DesktopStyle::Ios, dark: Some("ios-dark"), resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "ios-dark", label: "iOS dark", family: DesktopStyle::Ios, dark: None, resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "android", label: "Android", family: DesktopStyle::Android, dark: Some("android-dark"), resources: crate::widgets_dir),
    sheet_entry!(dir: "../themes", id: "android-dark", label: "Android dark", family: DesktopStyle::Android, dark: None, resources: crate::widgets_dir),
];

/// One registered set of sheets, and the set registered before it. The list
/// only grows, from its head, and a node is never freed: readers walk it with
/// no lock, from any thread, with or without a `Cx`.
struct CatalogueNode {
    sheets: &'static [SheetEntry],
    next: *const CatalogueNode,
}
unsafe impl Sync for CatalogueNode {}

static DESKTOP_NODE: CatalogueNode = CatalogueNode { sheets: &DESKTOP_SHEETS, next: std::ptr::null() };
static CATALOGUE: AtomicPtr<CatalogueNode> = AtomicPtr::new(&DESKTOP_NODE as *const CatalogueNode as *mut CatalogueNode);

fn catalogue_sets() -> Vec<&'static [SheetEntry]> {
    let mut sets = Vec::new();
    let mut at = CATALOGUE.load(Ordering::Acquire) as *const CatalogueNode;
    while !at.is_null() {
        // Safety: every node is a static or leaked, and never freed or changed
        // after it was published.
        let node = unsafe { &*at };
        sets.push(node.sheets);
        at = node.next;
    }
    sets.reverse();
    sets
}

/// Add an app's own sheets to the catalogue, after those already there. Call
/// it at startup, before the first style load; registering the same set again
/// changes nothing.
pub fn register(sheets: &'static [SheetEntry]) {
    let node = Box::leak(Box::new(CatalogueNode { sheets, next: std::ptr::null() }));
    loop {
        let head = CATALOGUE.load(Ordering::Acquire);
        if catalogue_sets().iter().any(|set| std::ptr::eq(*set, sheets)) {
            return;
        }
        node.next = head;
        if CATALOGUE
            .compare_exchange(head, node as *mut CatalogueNode, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return;
        }
    }
}

/// Every sheet in the catalogue, in the order the sets were registered (the
/// library's own first), each followed by its dark variant where the set
/// lists it so.
pub fn catalogue() -> Vec<&'static SheetEntry> {
    catalogue_sets().into_iter().flat_map(|set| set.iter()).collect()
}

/// The sheets a picker offers: every sheet that is not another's dark
/// variant, which the appearance toggle reaches instead.
pub fn picks() -> Vec<&'static SheetEntry> {
    let all = catalogue();
    all.iter()
        .copied()
        .filter(|entry| !all.iter().any(|other| other.dark == Some(entry.id)))
        .collect()
}

/// The sheet of this id, the first registered where two share one.
pub fn find(id: &str) -> Option<&'static SheetEntry> {
    catalogue_sets().into_iter().flat_map(|set| set.iter()).find(|entry| entry.id == id)
}

/// Two phases: theme tokens before widget registration, component overrides
/// after it. Applications then evaluate their own Splash against those defaults.
/// It is complete on its own: a process that receives one (a hosted app from
/// the WM) installs it without the sheet being in its own catalogue.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct StyleSheet {
    pub name: String,
    /// The family it lays out as, and whose icons it carries.
    pub family: DesktopStyle,
    /// The manifest directory its `crate_resource("self:...")` resolves
    /// against.
    pub resources: String,
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
    /// The sheet as it stands: a source checkout hotloads it from its source
    /// directory on every load; installed and wasm builds carry the identical
    /// embedded sources, with no external dependency.
    pub fn load(entry: &SheetEntry) -> Self {
        let read = |file: &str, bundled: &str| {
            #[cfg(not(target_arch = "wasm32"))]
            if let Ok(text) = std::fs::read_to_string(std::path::Path::new(entry.source_dir).join(entry.id).join(file)) {
                return text;
            }
            let _ = file;
            bundled.to_string()
        };
        Self {
            name: entry.id.into(),
            family: entry.family,
            resources: (entry.resources)().into(),
            theme: read("theme.splash", entry.theme),
            widgets: read("widgets.splash", entry.widgets),
            icons: crate::app_icon::load_assets(entry.family),
        }
    }
    /// The catalogue's sheet of this id.
    pub fn named(id: &str) -> Option<Self> {
        find(id).map(Self::load)
    }
    /// A colour token the sheet's token half assigns as a literal
    /// (`mod.theme.color_bg_app = #cacacc`), as `0xRRGGBBAA`: what a host
    /// that does not run the sheet (the WM's desktop ground, which is the
    /// window ground `theme.color_bg_app`) reads off it. The last assignment
    /// wins, as it does when the sheet runs.
    pub fn theme_color(&self, key: &str) -> Option<u32> {
        let prefix = format!("mod.theme.{key}");
        self.theme.lines().rev().find_map(|line| {
            let rest = line.trim().strip_prefix(&prefix)?.trim_start().strip_prefix('=')?;
            let hex = rest.split("//").next()?.trim().strip_prefix('#')?;
            let hex = hex.strip_prefix('x').unwrap_or(hex);
            let value = u32::from_str_radix(hex, 16).ok()?;
            match hex.len() {
                6 => Some((value << 8) | 0xff),
                8 => Some(value),
                _ => None,
            }
        })
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
    crate::app_icon::install(vm.cx_mut(), sheet.family, &sheet.icons);
    let key = vm.bx.heap.heap_key();
    vm.cx_mut().global::<Styles>().heaps.insert(key, sheet);
}
/// Take the sheet off again, so the next evaluation runs under the plain
/// theme. `install` had no way back: an app that lets somebody try a sheet
/// could put one on and never return to what it started with. A sheet named
/// by `MAKEPAD_WIDGET_STYLE` (or `MAKEPAD_WIDGET_STYLE_FILE`) comes back on the next read, as it would have
/// arrived in the first place.
pub fn uninstall(vm: &mut ScriptVm) {
    let key = vm.bx.heap.heap_key();
    vm.cx_mut().global::<Styles>().heaps.remove(&key);
}
/// The sheet installed on the heap `key`, if any: what a VM allocated from
/// that one inherits.
pub(crate) fn sheet_of_heap(cx: &mut Cx, key: usize) -> Option<StyleSheet> {
    cx.global::<Styles>().heaps.get(&key).cloned()
}
pub fn current(vm: &mut ScriptVm) -> Option<StyleSheet> {
    let key = vm.bx.heap.heap_key();
    if let Some(sheet) = vm.cx_mut().global::<Styles>().heaps.get(&key).cloned() {
        return Some(sheet);
    }
    // Opt-in only. Picking a sheet from OsType restyled every app that had
    // never asked for one, and an app that calls `theme_mod` + `widgets_mod`
    // without `script_mod` got the theme half of it and not the widget half.
    let sheet = env_sheet()?;
    install(vm, sheet.clone());
    Some(sheet)
}
/// The sheet the environment names. `MAKEPAD_WIDGET_STYLE_FILE` is a
/// `StyleSheet::to_json` file, so a launcher hands an app a sheet the app
/// never registered (Stage's app runs launch other apps under Stage's
/// sheets); it wins over `MAKEPAD_WIDGET_STYLE`, a catalogue id.
fn env_sheet() -> Option<StyleSheet> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = std::env::var_os("MAKEPAD_WIDGET_STYLE_FILE") {
        match std::fs::read_to_string(&path).ok().as_deref().and_then(StyleSheet::parse) {
            Some(sheet) => return Some(sheet),
            None => error!("MAKEPAD_WIDGET_STYLE_FILE: no style sheet in {}", std::path::Path::new(&path).display()),
        }
    }
    StyleSheet::named(&std::env::var("MAKEPAD_WIDGET_STYLE").ok()?)
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
        return sheet.family;
    }
    current(vm).map(|sheet| sheet.family).unwrap_or_default()
}
fn evaluate(vm: &mut ScriptVm, sheet: &StyleSheet, phase: &str, code: String) {
    vm.eval(ScriptMod {
        cargo_manifest_path: sheet.resources.clone(),
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
        if sheet.family.mobile() {
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
/// into the file. A line that opens a block -- a replaced `vertex` or
/// `pixel` function, an object -- takes the block with it, up to the line
/// that closes it, or its body would be left standing at the top level.
fn without_unregistered_widgets(vm: &mut ScriptVm, code: &str) -> String {
    let widgets = vm.module(id!(widgets));
    let registered = |vm: &mut ScriptVm, name: &str| {
        let value = vm.bx.heap.value(widgets, LiveId::from_str(name).into(), NoTrap);
        !(value.is_nil() || value.is_err())
    };
    // How far a line opens (or closes) brackets, its comment left out.
    let depth = |line: &str| -> i32 {
        let code = line.split("//").next().unwrap_or("");
        code.chars()
            .map(|c| match c {
                '{' | '(' | '[' => 1,
                '}' | ')' | ']' => -1,
                _ => 0,
            })
            .sum()
    };
    let mut out: Vec<&str> = Vec::new();
    // The brackets still open in a statement being blanked.
    let mut open = 0;
    for line in code.split('\n') {
        if open > 0 {
            open += depth(line);
            out.push("");
            continue;
        }
        let missing = line.match_indices("mod.widgets.").any(|(at, prefix)| {
            let name: String = line[at + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            !name.is_empty() && !registered(vm, &name)
        });
        if missing {
            open = depth(line).max(0);
            out.push("");
        } else {
            out.push(line);
        }
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
        family: DesktopStyle::Omarchy,
        resources: crate::widgets_dir().into(),
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

    use crate::sheet_checks;

    /// A row of fields is one height, under every sheet in the catalogue.
    #[test]
    fn every_field_stands_at_the_height_its_sheet_gives_the_text_box() {
        for entry in catalogue() {
            sheet_checks::field_row(crate::script_mod, entry);
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
                    install(vm,StyleSheet::load(style.sheet(dark)));
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
    /// Every face a sheet gives the app keeps the app's fallbacks behind it,
    /// under every sheet in the catalogue.
    #[test]
    fn every_sheet_keeps_the_fallbacks_behind_its_own_face() {
        let checked: usize = catalogue().into_iter().map(|entry| sheet_checks::font_fallbacks(crate::script_mod, entry)).sum();
        assert!(checked > 50, "only {checked} faces were read");
    }
    #[test]
    fn every_sheet_evaluates_and_ends_both_halves_in_true() {
        for entry in catalogue() {
            sheet_checks::evaluates(crate::script_mod, entry);
        }
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
                install(vm, StyleSheet::load(style.sheet(false)));
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
                install(vm, StyleSheet::load(style.sheet(dark)));
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
            install(vm, StyleSheet::load(DesktopStyle::Windows.sheet(false)));
            vm.with_reload(crate::script_mod);
            panel.script_apply(vm, &Apply::ScriptReapply, &mut Scope::empty(), original);
            assert!(panel.visible, "An open panel must survive a style change");
            panel.script_apply(vm, &Apply::Eval, &mut Scope::empty(), original);
            assert!(!panel.visible, "Explicit visibility edits must still apply");
        });
    }
    /// A table indexed by discriminant is `COUNT` long, and every variant
    /// is in `ALL` exactly once, so no family falls off the end of one.
    #[test]
    fn every_style_is_listed_once_and_fits_the_count() {
        for (at, style) in DesktopStyle::ALL.into_iter().enumerate() {
            assert!((style as usize) < DesktopStyle::COUNT, "{style:?}");
            assert_eq!(DesktopStyle::ALL.iter().position(|s| *s == style), Some(at), "{style:?} twice");
            assert_eq!(DesktopStyle::parse(style.id()), Some(style));
        }
        // The tilers have no shelf and no title bar of their own.
        for style in DesktopStyle::TILING {
            assert!(!style.floating(), "{style:?} floats");
            assert_eq!(style.shelf_height(), 0.0);
            assert_eq!(style.title_height(), 0.0);
        }
    }

    /// Every family has its own sheet, of its own family, with a dark variant
    /// exactly where the family has a dark appearance; no two sheets share an
    /// id, and every dark variant named is in the catalogue, of the same
    /// family, and offers no dark variant of its own.
    #[test]
    fn the_catalogue_holds_together() {
        let all = catalogue();
        for (at, entry) in all.iter().enumerate() {
            assert!(!all[..at].iter().any(|other| other.id == entry.id), "{} is registered twice", entry.id);
            if let Some(dark) = entry.dark {
                let variant = find(dark).unwrap_or_else(|| panic!("{}: its dark variant {dark} is not in the catalogue", entry.id));
                assert_eq!(variant.family, entry.family, "{dark}");
                assert!(variant.dark.is_none(), "{dark} has a dark variant of its own");
                assert!(variant.is_dark_variant() && std::ptr::eq(variant.light(), *entry));
                assert!(std::ptr::eq(entry.with_appearance(true), variant));
                assert!(std::ptr::eq(variant.with_appearance(false), *entry));
            }
        }
        for style in DesktopStyle::ALL {
            let own = style.sheet(false);
            assert_eq!((own.id, own.family), (style.id(), style));
            assert_eq!(own.dark.is_some(), style.supports_dark(), "{}", style.id());
        }
        assert_eq!(picks().len() + all.iter().filter(|entry| entry.is_dark_variant()).count(), all.len());
    }

    /// A sheet lays a window ground with two lines, and a window under a
    /// sheet that writes neither keeps its ground off, so it only clears as
    /// it always did. Read off a window made from the template the way an
    /// app makes one, since that is where the sheet's writes have to land.
    #[test]
    fn a_sheet_lays_the_window_ground_and_only_a_sheet_that_asks_for_one() {
        for entry in catalogue() {
            sheet_checks::window_ground(crate::script_mod, entry);
        }
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            let shows = |vm: &mut ScriptVm| {
                let window = script_eval!(vm, {mod.widgets.Window{}});
                let obj = window.as_object().expect("a window object");
                vm.bx.heap.value(obj, id!(show_bg).into(), NoTrap).as_bool()
            };
            assert_eq!(shows(vm), Some(false), "the stock window draws no ground");
            let ground = StyleSheet {
                name: "ground".into(),
                family: DesktopStyle::Omarchy,
                resources: crate::widgets_dir().into(),
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
            install(vm, StyleSheet::load(DesktopStyle::Macos.sheet(false)));
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
                family: DesktopStyle::Omarchy,
                resources: crate::widgets_dir().into(),
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
            assert!(sheet.widgets.lines().count() > 5_000, "the sheet sets only {} leaves", sheet.widgets.lines().count());
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
    fn a_host_reads_a_literal_colour_off_the_token_half() {
        let sheet = StyleSheet::named("windows-2000").unwrap();
        assert_eq!(sheet.theme_color("color_bg_app"), Some(0xd4d0c8ff));
        assert_eq!(sheet.theme_color("color_no_such_token"), None);
    }

    #[test]
    fn stylesheet_wire_preserves_both_splash_phases() {
        let sheet = StyleSheet::load(DesktopStyle::Windows2000.sheet(false));
        assert_eq!(StyleSheet::parse(&sheet.to_json()), Some(sheet));
        assert!(StyleSheet::parse("{\"wm\":\"Adopted\"}").is_none());
    }
}
