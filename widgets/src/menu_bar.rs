//! `MenuBar` — the in-app application menu bar.
//!
//! A horizontal row of menu titles; clicking a title drops its menu open
//! directly underneath, in an overlay draw list so it paints above every
//! other widget (the same technique `PopupMenu` uses for `DropDown`).
//! Entries carry a label on the left and their keyboard shortcut, muted and
//! right-aligned, on the right; `{sep: true}` draws a 1 px rule.
//!
//! The menu tree is data, not widgets — the host writes it as a plain script
//! array and the bar parses it in `on_after_apply`:
//!
//! ```text
//! menu_bar := MenuBar{
//!     menus: [
//!         {label: "File" items: [
//!             {id: @new_from_template label: "New from template…" shortcut: "Cmd+N"}
//!             {id: @open_flow label: "Open flow" shortcut: "Cmd+O"}
//!             {sep: true}
//!             {id: @quit label: "Quit" shortcut: "Cmd+Q"}
//!         ]}
//!         {label: "Edit" items: [
//!             {id: @undo label: "Undo" shortcut: "Cmd+Z"}
//!             {id: @redo label: "Redo" shortcut: "Shift+Cmd+Z"}
//!         ]}
//!     ]
//! }
//! ```
//!
//! `shortcut` and `enabled` are optional (`enabled: false` greys an entry and
//! retires its shortcut). A host that builds its menus in Rust calls
//! [`MenuBar::set_menus`] instead, and [`MenuBar::set_enabled`] to grey one
//! entry by id. Whichever way an entry fires — click or shortcut — the bar
//! publishes [`MenuBarAction::Selected`] with that entry's id, which the host
//! reads with [`MenuBarRef::selected`].
//!
//! Shortcuts are matched against every `Event::KeyDown` regardless of focus,
//! and only when an entry actually claims the chord: keys nothing matches are
//! left entirely alone. `Cmd` means the command/logo modifier on macOS and
//! Ctrl everywhere else; the display maps `Cmd`/`Shift`/`Alt`/`Ctrl` to
//! ⌘/⇧/⌥/⌃ and prints the key token as written.

use crate::makepad_script::trap::NoTrap;
use crate::makepad_script::ScriptObject;
use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.fab_internal.*
    use mod.widgets.*

    /** The title pill: transparent at rest, a light pill under the pointer,
     * the accent pill while its menu is open. */
    set_type_default() do #(DrawMenuTitle::script_shader(vm)){
        ..mod.draw.DrawQuad
        hover: 0.0
        open: 0.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius_lg)
            let rest = vec4(fab.color_row_hover.xyz, fab.color_row_hover.w * self.hover)
            sdf.fill(rest.mix(fab.color_accent, self.open))
            return sdf.result
        }
    }

    /** The entry row highlight: the accent, faded in by the pointer. */
    set_type_default() do #(DrawMenuEntry::script_shader(vm)){
        ..mod.draw.DrawQuad
        hover: 0.0
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius)
            sdf.fill(vec4(fab.color_accent.xyz, fab.color_accent.w * self.hover))
            return sdf.result
        }
    }

    /** Menu ink: the label at rest, muted for shortcuts, on-accent while its
     * title is open, muted again when the entry is disabled. */
    set_type_default() do #(DrawMenuText::script_shader(vm)){
        ..mod.draw.DrawText
        open: 0.0
        muted: 0.0
        disabled: 0.0
        get_color: fn() {
            return self.color
                .mix(fab.color_text_muted, self.muted)
                .mix(fab.color_text_on_accent, self.open)
                .mix(fab.color_text_muted, self.disabled)
        }
    }

    mod.widgets.MenuBarBase = #(MenuBar::register_widget(vm))

    /** The application menu bar: a row of titles, each dropping an overlay
     * menu of entries with shortcuts. */
    mod.widgets.MenuBar = set_type_default() do mod.widgets.MenuBarBase{
        width: Fill
        height: 28

        /** the bar's height when the walk does not fix one 20..48 step 1 */
        bar_height: 28.0
        /** gap from the bar's left edge to the first title 0..32 step 1 */
        bar_pad_x: 8.0
        /** the title pill's height 16..40 step 1 */
        title_height: 20.0
        /** ink inset inside a title pill 4..24 step 1 */
        title_pad_x: 8.0
        /** gap between two title pills 0..16 step 1 */
        title_gap: 2.0
        /** one menu entry's row height 16..40 step 1 */
        item_height: 22.0
        /** ink inset inside a menu entry 4..24 step 1 */
        item_pad_x: 10.0
        /** the drop-down panel's own inset 0..16 step 1 */
        panel_pad: 4.0
        /** the drop-down panel never narrows past this 80..400 step 8 */
        panel_min_width: 160.0
        /** clearance between a label and its shortcut 8..64 step 4 */
        shortcut_gap: 24.0
        /** the row a `{sep: true}` entry occupies 3..24 step 1 */
        separator_height: 7.0

        /** The bar surface: the panel grade with a hairline under it. */
        draw_bg +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                sdf.fill(fab.color_panel)
                sdf.rect(0.0, self.rect_size.y - 1.0, self.rect_size.x, 1.0)
                sdf.fill(fab.color_border)
                return sdf.result
            }
        }

        /** The drop-down panel: the popover grade with a 1 px border. */
        draw_panel +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, fab.radius_lg)
                sdf.fill_keep(fab.color_popover)
                sdf.stroke(fab.color_popover_border, 1.0)
                return sdf.result
            }
        }

        draw_sep +: {
            color: fab.color_border_light
        }

        draw_title_text +: {
            color: fab.color_text
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }

        draw_entry_text +: {
            color: fab.color_text
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }

        draw_shortcut_text +: {
            muted: 1.0
            color: fab.color_text
            text_style: theme.font_regular{
                font_size: fab.font_size_ui
            }
        }
    }
}

// ---------------------------------------------------------------------------
// the menu tree, as Rust
// ---------------------------------------------------------------------------

/// One menu on the bar: a title and the entries it drops.
#[derive(Clone, Debug, Default)]
pub struct MenuDef {
    pub label: String,
    pub items: Vec<MenuEntry>,
}

impl MenuDef {
    pub fn new(label: impl Into<String>, items: Vec<MenuEntry>) -> Self {
        Self {
            label: label.into(),
            items,
        }
    }
}

/// One line in an open menu: either an entry that fires, or a separator.
#[derive(Clone, Debug)]
pub struct MenuEntry {
    /// The id published as [`MenuBarAction::Selected`] when this entry fires.
    pub id: LiveId,
    pub label: String,
    /// The chord as written (`"Shift+Cmd+Z"`), not yet parsed or prettied.
    pub shortcut: Option<String>,
    parsed_shortcut: Option<MenuShortcut>,
    /// A disabled entry greys out, ignores the pointer and drops its chord.
    pub enabled: bool,
    /// A 1 px rule; `id`, `label` and `shortcut` are unused.
    pub separator: bool,
}

impl Default for MenuEntry {
    fn default() -> Self {
        Self {
            id: LiveId(0),
            label: String::new(),
            shortcut: None,
            parsed_shortcut: None,
            enabled: true,
            separator: false,
        }
    }
}

impl MenuEntry {
    pub fn item(id: LiveId, label: impl Into<String>, shortcut: Option<&str>) -> Self {
        let shortcut = shortcut.map(str::to_string);
        Self {
            id,
            label: label.into(),
            parsed_shortcut: shortcut.as_deref().and_then(parse_shortcut),
            shortcut,
            ..Self::default()
        }
    }

    pub fn separator() -> Self {
        Self {
            separator: true,
            ..Self::default()
        }
    }
}

/// What the bar publishes to its host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MenuBarAction {
    /// An entry fired — by click or by its keyboard shortcut.
    Selected(LiveId),
    Opened,
    Closed,
    #[default]
    None,
}

// ---------------------------------------------------------------------------
// shortcuts
// ---------------------------------------------------------------------------

/// A parsed chord. `cmd` is the command/logo modifier on macOS and Ctrl
/// everywhere else; `ctrl` is always the literal Control key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuShortcut {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// `Plus` sits behind Shift on many layouts and in front of it on
    /// others, so that one token matches either way.
    pub shift_any: bool,
    pub key: KeyCode,
}

impl MenuShortcut {
    /// Does this key press fire the chord? Modifiers must match exactly, so
    /// `Cmd+N` stays put while `Shift+Cmd+N` is pressed.
    pub fn matches(&self, ke: &KeyEvent) -> bool {
        if ke.key_code != self.key {
            return false;
        }
        let m = &ke.modifiers;
        if m.alt != self.alt {
            return false;
        }
        if !self.shift_any && m.shift != self.shift {
            return false;
        }
        #[cfg(target_arch = "wasm32")]
        {
            // The browser hands mac users logo and everyone else control;
            // either one stands in for `Cmd`.
            (m.logo || m.control) == (self.cmd || self.ctrl)
        }
        #[cfg(all(not(target_arch = "wasm32"), target_vendor = "apple"))]
        {
            m.logo == self.cmd && m.control == self.ctrl
        }
        #[cfg(all(not(target_arch = "wasm32"), not(target_vendor = "apple")))]
        {
            m.control == (self.cmd || self.ctrl) && !m.logo
        }
    }
}

/// Parse `"Shift+Cmd+Z"`, `"Delete"`, `"Cmd+Plus"`, `"F1"`, `"Cmd+0"` …
/// `None` when the key token is not one this keymap knows — such a shortcut
/// simply never fires (it still prints).
pub fn parse_shortcut(text: &str) -> Option<MenuShortcut> {
    let mut out = MenuShortcut::default();
    let mut key = None;
    for part in text.split('+') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "cmd" | "command" | "meta" | "super" | "win" => out.cmd = true,
            "ctrl" | "control" => out.ctrl = true,
            "alt" | "option" | "opt" => out.alt = true,
            "shift" => out.shift = true,
            other => {
                if key.is_some() {
                    return None;
                }
                key = Some(key_code_from_name(other)?);
                out.shift_any = other == "plus";
            }
        }
    }
    out.key = key?;
    Some(out)
}

/// The chord as a menu prints it: ⌘ / ⇧ / ⌥ / ⌃ for the modifiers, the key
/// token verbatim, no separators.
pub fn shortcut_display(text: &str) -> String {
    let mut out = String::new();
    for part in text.split('+') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" | "super" | "win" => out.push('\u{2318}'),
            "shift" => out.push('\u{21e7}'),
            "alt" | "option" | "opt" => out.push('\u{2325}'),
            "ctrl" | "control" => out.push('\u{2303}'),
            "plus" => out.push('+'),
            "minus" => out.push('-'),
            _ => out.push_str(part),
        }
    }
    out
}

/// `name` arrives lowercased.
fn key_code_from_name(name: &str) -> Option<KeyCode> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if let Some(key) = key_code_from_char(c) {
            return Some(key);
        }
    }
    Some(match name {
        "f1" => KeyCode::F1,
        "f2" => KeyCode::F2,
        "f3" => KeyCode::F3,
        "f4" => KeyCode::F4,
        "f5" => KeyCode::F5,
        "f6" => KeyCode::F6,
        "f7" => KeyCode::F7,
        "f8" => KeyCode::F8,
        "f9" => KeyCode::F9,
        "f10" => KeyCode::F10,
        "f11" => KeyCode::F11,
        "f12" => KeyCode::F12,
        "escape" | "esc" => KeyCode::Escape,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Space,
        "enter" | "return" => KeyCode::ReturnKey,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "left" | "arrowleft" => KeyCode::ArrowLeft,
        "right" | "arrowright" => KeyCode::ArrowRight,
        "up" | "arrowup" => KeyCode::ArrowUp,
        "down" | "arrowdown" => KeyCode::ArrowDown,
        "plus" | "equals" | "equal" => KeyCode::Equals,
        "minus" | "dash" => KeyCode::Minus,
        "comma" => KeyCode::Comma,
        "period" | "dot" => KeyCode::Period,
        "slash" => KeyCode::Slash,
        "backslash" => KeyCode::Backslash,
        "semicolon" => KeyCode::Semicolon,
        "quote" => KeyCode::Quote,
        "backtick" | "grave" => KeyCode::Backtick,
        "lbracket" => KeyCode::LBracket,
        "rbracket" => KeyCode::RBracket,
        _ => return None,
    })
}

fn key_code_from_char(c: char) -> Option<KeyCode> {
    Some(match c.to_ascii_lowercase() {
        'a' => KeyCode::KeyA,
        'b' => KeyCode::KeyB,
        'c' => KeyCode::KeyC,
        'd' => KeyCode::KeyD,
        'e' => KeyCode::KeyE,
        'f' => KeyCode::KeyF,
        'g' => KeyCode::KeyG,
        'h' => KeyCode::KeyH,
        'i' => KeyCode::KeyI,
        'j' => KeyCode::KeyJ,
        'k' => KeyCode::KeyK,
        'l' => KeyCode::KeyL,
        'm' => KeyCode::KeyM,
        'n' => KeyCode::KeyN,
        'o' => KeyCode::KeyO,
        'p' => KeyCode::KeyP,
        'q' => KeyCode::KeyQ,
        'r' => KeyCode::KeyR,
        's' => KeyCode::KeyS,
        't' => KeyCode::KeyT,
        'u' => KeyCode::KeyU,
        'v' => KeyCode::KeyV,
        'w' => KeyCode::KeyW,
        'x' => KeyCode::KeyX,
        'y' => KeyCode::KeyY,
        'z' => KeyCode::KeyZ,
        '0' => KeyCode::Key0,
        '1' => KeyCode::Key1,
        '2' => KeyCode::Key2,
        '3' => KeyCode::Key3,
        '4' => KeyCode::Key4,
        '5' => KeyCode::Key5,
        '6' => KeyCode::Key6,
        '7' => KeyCode::Key7,
        '8' => KeyCode::Key8,
        '9' => KeyCode::Key9,
        '+' | '=' => KeyCode::Equals,
        '-' => KeyCode::Minus,
        ',' => KeyCode::Comma,
        '.' => KeyCode::Period,
        '/' => KeyCode::Slash,
        '\\' => KeyCode::Backslash,
        ';' => KeyCode::Semicolon,
        '\'' => KeyCode::Quote,
        '`' => KeyCode::Backtick,
        '[' => KeyCode::LBracket,
        ']' => KeyCode::RBracket,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// draw shaders
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMenuTitle {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
    #[live]
    open: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMenuEntry {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    hover: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawMenuText {
    #[deref]
    draw_super: DrawText,
    #[live]
    open: f32,
    #[live]
    muted: f32,
    #[live]
    disabled: f32,
}

// ---------------------------------------------------------------------------
// the widget
// ---------------------------------------------------------------------------

#[derive(Script, Widget)]
pub struct MenuBar {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,

    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_title: DrawMenuTitle,
    #[live]
    draw_title_text: DrawMenuText,
    #[live]
    draw_panel: DrawQuad,
    #[live]
    draw_entry: DrawMenuEntry,
    #[live]
    draw_entry_text: DrawMenuText,
    #[live]
    draw_shortcut_text: DrawMenuText,
    #[live]
    draw_sep: DrawColor,

    /// The host's menu tree, as written in the DSL. Read once per apply in
    /// [`ScriptHook::on_after_apply`] and never dereferenced afterwards.
    #[live]
    menus: ScriptValue,

    #[live(28.0)]
    bar_height: f64,
    #[live(8.0)]
    bar_pad_x: f64,
    #[live(20.0)]
    title_height: f64,
    #[live(8.0)]
    title_pad_x: f64,
    #[live(2.0)]
    title_gap: f64,
    #[live(22.0)]
    item_height: f64,
    #[live(10.0)]
    item_pad_x: f64,
    #[live(4.0)]
    panel_pad: f64,
    #[live(160.0)]
    panel_min_width: f64,
    #[live(24.0)]
    shortcut_gap: f64,
    #[live(7.0)]
    separator_height: f64,

    #[rust]
    defs: Vec<MenuDef>,
    /// The raw `menus` value the current [`Self::defs`] were parsed from, so
    /// an unrelated re-apply does not wipe a runtime [`MenuBar::set_menus`].
    #[rust]
    menus_source: ScriptValue,
    #[rust]
    overlay_list: Option<DrawList2d>,

    #[rust]
    open_menu: Option<usize>,
    #[rust]
    hot_title: Option<usize>,
    #[rust]
    hot_entry: Option<usize>,
    /// Focus restored when the menu closes.
    #[rust]
    focus_before_open: Area,

    #[rust]
    bar_rect: Rect,
    #[rust]
    title_rects: Vec<Rect>,
    /// The open panel's UNCLIPPED rect — the outside-press test needs the
    /// overlay's real bounds, not a rect intersected with the host's clips.
    #[rust]
    panel_rect: Rect,
    #[rust]
    entry_rects: Vec<Rect>,
}

impl ScriptHook for MenuBar {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.overlay_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.menus.raw() != self.menus_source.raw() {
            self.menus_source = self.menus;
            self.defs = parse_menus(vm, self.menus);
            self.open_menu = None;
            self.hot_title = None;
            self.hot_entry = None;
        }
        let cx = vm.cx_mut();
        self.draw_bg.redraw(cx);
    }
}

// ---------------------------------------------------------------------------
// reading the script tree
// ---------------------------------------------------------------------------

/// Walk a script array, whether it arrived as an array value or as an
/// object carrying a vec (both spellings reach a `#[live] ScriptValue`).
fn for_each_element(
    vm: &mut ScriptVm,
    value: ScriptValue,
    f: &mut dyn FnMut(&mut ScriptVm, ScriptValue),
) {
    if let Some(array) = value.as_array() {
        let len = vm.bx.heap.array_len(array);
        for i in 0..len {
            let item = vm.bx.heap.array_index_unchecked(array, i);
            f(vm, item);
        }
        return;
    }
    if let Some(object) = value.as_object() {
        let len = vm.bx.heap.vec_len(object);
        for i in 0..len {
            if let Some(item) = vm.bx.heap.vec_value_if_exist(object, i) {
                f(vm, item);
            }
        }
    }
}

fn obj_field(vm: &mut ScriptVm, object: ScriptObject, key: LiveId) -> ScriptValue {
    let value = vm.bx.heap.value(object, key.into(), NoTrap);
    if value.is_err() {
        ScriptValue::NIL
    } else {
        value
    }
}

fn obj_string(vm: &mut ScriptVm, object: ScriptObject, key: LiveId) -> Option<String> {
    let value = obj_field(vm, object, key);
    if value.is_nil() {
        return None;
    }
    vm.string_with(value, |_, s| s.to_string())
}

fn obj_bool(vm: &mut ScriptVm, object: ScriptObject, key: LiveId) -> Option<bool> {
    obj_field(vm, object, key).as_bool()
}

fn parse_menus(vm: &mut ScriptVm, value: ScriptValue) -> Vec<MenuDef> {
    let mut menus = Vec::new();
    for_each_element(vm, value, &mut |vm, menu| {
        let Some(menu_obj) = menu.as_object() else {
            return;
        };
        let label = obj_string(vm, menu_obj, id!(label)).unwrap_or_default();
        let items_value = obj_field(vm, menu_obj, id!(items));
        let mut items = Vec::new();
        for_each_element(vm, items_value, &mut |vm, entry| {
            let Some(entry_obj) = entry.as_object() else {
                return;
            };
            if obj_bool(vm, entry_obj, id!(sep)).unwrap_or(false) {
                items.push(MenuEntry::separator());
                return;
            }
            // An entry with no id can never be reported, so it is not an entry.
            let Some(id) = obj_field(vm, entry_obj, id!(id)).as_id() else {
                return;
            };
            items.push(MenuEntry {
                id,
                label: obj_string(vm, entry_obj, id!(label)).unwrap_or_default(),
                shortcut: obj_string(vm, entry_obj, id!(shortcut)).filter(|s| !s.is_empty()),
                parsed_shortcut: None,
                enabled: obj_bool(vm, entry_obj, id!(enabled)).unwrap_or(true),
                separator: false,
            });
        });
        for entry in &mut items {
            entry.parsed_shortcut = entry.shortcut.as_deref().and_then(parse_shortcut);
        }
        menus.push(MenuDef { label, items });
    });
    menus
}

// ---------------------------------------------------------------------------
// drawing
// ---------------------------------------------------------------------------

/// One line of text's drawn size. Free-standing so the caller can keep its
/// own `&mut self` while measuring against one of its draw layers.
fn measure(dt: &DrawMenuText, cx: &mut Cx2d, text: &str) -> DVec2 {
    if text.is_empty() {
        return dvec2(0.0, 0.0);
    }
    let laidout = dt.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
    let scale = dt.font_scale as f64;
    dvec2(
        laidout.size_in_lpxs.width as f64 * scale,
        laidout.size_in_lpxs.height as f64 * scale,
    )
}

impl MenuBar {
    /// Replace the whole menu tree at runtime. Closes any open menu.
    pub fn set_menus(&mut self, cx: &mut Cx, menus: Vec<MenuDef>) {
        self.defs = menus;
        for menu in &mut self.defs {
            for entry in &mut menu.items {
                entry.parsed_shortcut = entry.shortcut.as_deref().and_then(parse_shortcut);
            }
        }
        self.open_menu = None;
        self.hot_title = None;
        self.hot_entry = None;
        self.panel_rect = Rect::default();
        self.draw_bg.redraw(cx);
        // The overlay has to be re-laid even when it is going away.
        cx.redraw_all();
    }

    /// Grey (or restore) every entry carrying `id`. A disabled entry ignores
    /// the pointer and drops its keyboard shortcut.
    pub fn set_enabled(&mut self, cx: &mut Cx, id: LiveId, enabled: bool) {
        let mut changed = false;
        for menu in &mut self.defs {
            for entry in &mut menu.items {
                if !entry.separator && entry.id == id && entry.enabled != enabled {
                    entry.enabled = enabled;
                    changed = true;
                }
            }
        }
        if changed {
            self.draw_bg.redraw(cx);
            self.redraw_overlay(cx);
        }
    }

    pub fn menus(&self) -> &[MenuDef] {
        &self.defs
    }

    pub fn is_open(&self) -> bool {
        self.open_menu.is_some()
    }

    pub fn open(&mut self, cx: &mut Cx, index: usize) {
        if index >= self.defs.len() || self.open_menu == Some(index) {
            return;
        }
        if self.open_menu.is_none() {
            self.focus_before_open = cx.key_focus();
        }
        self.open_menu = Some(index);
        self.hot_entry = self.first_enabled(index);
        cx.set_key_focus(self.draw_bg.area());
        let uid = self.widget_uid();
        cx.widget_action(uid, MenuBarAction::Opened);
        self.redraw_overlay(cx);
        self.draw_bg.redraw(cx);
    }

    pub fn close(&mut self, cx: &mut Cx) {
        if self.open_menu.take().is_none() {
            return;
        }
        self.hot_entry = None;
        self.panel_rect = Rect::default();
        let uid = self.widget_uid();
        cx.widget_action(uid, MenuBarAction::Closed);
        self.redraw_overlay(cx);
        self.draw_bg.redraw(cx);
        if cx.key_focus() == self.draw_bg.area() {
            cx.set_key_focus(self.focus_before_open);
        }
        // The overlay draw list keeps painting until the whole pass is
        // rebuilt, so a close is a full redraw, exactly like FabColorPick's.
        cx.redraw_all();
    }

    fn redraw_overlay(&self, cx: &mut Cx) {
        if let Some(list) = &self.overlay_list {
            list.redraw(cx);
        }
    }

    fn fire(&mut self, cx: &mut Cx, id: LiveId) {
        let uid = self.widget_uid();
        cx.widget_action(uid, MenuBarAction::Selected(id));
    }

    fn title_at(&self, abs: DVec2) -> Option<usize> {
        self.title_rects.iter().position(|r| r.contains(abs))
    }

    /// The entry under `abs`, if it is one that can fire (separators and
    /// disabled entries are not hot).
    fn entry_at(&self, abs: DVec2) -> Option<usize> {
        let items = &self.defs.get(self.open_menu?)?.items;
        let index = self.entry_rects.iter().position(|r| r.contains(abs))?;
        let entry = items.get(index)?;
        (!entry.separator && entry.enabled).then_some(index)
    }

    fn match_shortcut(&self, ke: &KeyEvent) -> Option<LiveId> {
        for menu in &self.defs {
            for entry in &menu.items {
                if entry.separator || !entry.enabled {
                    continue;
                }
                let Some(shortcut) = entry.parsed_shortcut else {
                    continue;
                };
                if shortcut.matches(ke) {
                    return Some(entry.id);
                }
            }
        }
        None
    }

    fn first_enabled(&self, menu: usize) -> Option<usize> {
        self.defs.get(menu)?.items.iter().position(|entry| !entry.separator && entry.enabled)
    }

    fn step_entry(&self, menu: usize, current: Option<usize>, delta: isize) -> Option<usize> {
        let items = &self.defs.get(menu)?.items;
        if items.is_empty() {
            return None;
        }
        let mut index = current.unwrap_or(if delta > 0 { items.len() - 1 } else { 0 });
        for _ in 0..items.len() {
            index = (index as isize + delta).rem_euclid(items.len() as isize) as usize;
            if !items[index].separator && items[index].enabled {
                return Some(index);
            }
        }
        None
    }

    /// Accelerators are dispatched by the host after it has determined
    /// whether an editor owns focus. Returning true means a menu action was
    /// queued. Open-menu navigation remains inside the widget itself.
    pub fn handle_shortcut(&mut self, cx: &mut Cx, event: &Event, text_editing: bool) -> bool {
        let Event::KeyDown(ke) = event else {
            return false;
        };
        if text_editing || ke.is_repeat || self.open_menu.is_some() {
            return false;
        }
        let Some(id) = self.match_shortcut(ke) else {
            return false;
        };
        self.fire(cx, id);
        true
    }

    fn handle_open_key(&mut self, cx: &mut Cx, ke: &KeyEvent) -> bool {
        let Some(menu) = self.open_menu else {
            return false;
        };
        match ke.key_code {
            KeyCode::Escape => self.close(cx),
            KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                let delta = if ke.key_code == KeyCode::ArrowLeft { -1 } else { 1 };
                let next = (menu as isize + delta).rem_euclid(self.defs.len() as isize) as usize;
                self.open(cx, next);
            }
            KeyCode::ArrowUp => {
                self.hot_entry = self.step_entry(menu, self.hot_entry, -1);
                self.redraw_overlay(cx);
            }
            KeyCode::ArrowDown => {
                self.hot_entry = self.step_entry(menu, self.hot_entry, 1);
                self.redraw_overlay(cx);
            }
            KeyCode::ReturnKey | KeyCode::Space => {
                let id = self.hot_entry.and_then(|entry| {
                    self.defs
                        .get(menu)?
                        .items
                        .get(entry)
                        .filter(|entry| !entry.separator && entry.enabled)
                        .map(|entry| entry.id)
                });
                if let Some(id) = id {
                    self.close(cx);
                    self.fire(cx, id);
                }
            }
            _ => return false,
        }
        true
    }

    fn draw_titles(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.title_rects.clear();
        let title_height = self.title_height.min(rect.size.y);
        let y = rect.pos.y + (rect.size.y - title_height) * 0.5;
        let mut x = rect.pos.x + self.bar_pad_x;
        for index in 0..self.defs.len() {
            let label = self.defs[index].label.clone();
            let size = measure(&self.draw_title_text, cx, &label);
            let width = size.x + self.title_pad_x * 2.0;
            let title_rect = Rect {
                pos: dvec2(x, y),
                size: dvec2(width, title_height),
            };
            let open = self.open_menu == Some(index);
            self.draw_title.hover = if !open && self.hot_title == Some(index) {
                1.0
            } else {
                0.0
            };
            self.draw_title.open = if open { 1.0 } else { 0.0 };
            self.draw_title.draw_abs(cx, title_rect);
            self.draw_title_text.open = self.draw_title.open;
            self.draw_title_text.draw_abs(
                cx,
                dvec2(
                    title_rect.pos.x + self.title_pad_x,
                    title_rect.pos.y + (title_height - size.y) * 0.5,
                ),
                &label,
            );
            self.title_rects.push(title_rect);
            x += width + self.title_gap;
        }
    }

    /// The open menu, in an overlay draw list anchored under its title.
    fn draw_menu(&mut self, cx: &mut Cx2d, index: usize) {
        let Some(items) = self.defs.get(index).map(|m| m.items.clone()) else {
            return;
        };
        let Some(mut list) = self.overlay_list.take() else {
            return;
        };

        // Measure first: the panel is placed by the size it needs.
        let mut content_width: f64 = 0.0;
        let mut height = self.panel_pad * 2.0;
        for entry in &items {
            if entry.separator {
                height += self.separator_height;
                continue;
            }
            height += self.item_height;
            let label_width = measure(&self.draw_entry_text, cx, &entry.label).x;
            let shortcut_width = match &entry.shortcut {
                Some(text) => measure(&self.draw_shortcut_text, cx, &shortcut_display(text)).x,
                None => 0.0,
            };
            let row = label_width
                + if shortcut_width > 0.0 {
                    self.shortcut_gap + shortcut_width
                } else {
                    0.0
                };
            content_width = content_width.max(row);
        }
        let width =
            (content_width + (self.item_pad_x + self.panel_pad) * 2.0).max(self.panel_min_width);

        let pass = cx.current_pass_size();
        let anchor = self
            .title_rects
            .get(index)
            .copied()
            .unwrap_or(self.bar_rect);
        let mut pos = dvec2(anchor.pos.x, self.bar_rect.pos.y + self.bar_rect.size.y);
        // Flip above the bar when the drop would run off the bottom.
        if pos.y + height > pass.y {
            pos.y = (anchor.pos.y - height).max(0.0);
        }
        pos.x = pos.x.clamp(0.0, (pass.x - width).max(0.0));
        let panel_rect = Rect {
            pos,
            size: dvec2(width, height),
        };
        self.panel_rect = panel_rect;

        list.begin_overlay_reuse(cx);
        cx.begin_root_turtle(pass, Layout::flow_down());

        self.draw_panel.draw_abs(cx, panel_rect);

        self.entry_rects.clear();
        let mut y = pos.y + self.panel_pad;
        for (i, entry) in items.iter().enumerate() {
            if entry.separator {
                let row = Rect {
                    pos: dvec2(pos.x + self.item_pad_x, y),
                    size: dvec2(
                        (width - self.item_pad_x * 2.0).max(0.0),
                        self.separator_height,
                    ),
                };
                self.draw_sep.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(row.pos.x, (y + (self.separator_height - 1.0) * 0.5).floor()),
                        size: dvec2(row.size.x, 1.0),
                    },
                );
                self.entry_rects.push(row);
                y += self.separator_height;
                continue;
            }

            let row = Rect {
                pos: dvec2(pos.x + self.panel_pad, y),
                size: dvec2((width - self.panel_pad * 2.0).max(0.0), self.item_height),
            };
            self.draw_entry.hover = if self.hot_entry == Some(i) { 1.0 } else { 0.0 };
            self.draw_entry.draw_abs(cx, row);

            let disabled = if entry.enabled { 0.0 } else { 1.0 };
            self.draw_entry_text.disabled = disabled;
            let label_size = measure(&self.draw_entry_text, cx, &entry.label);
            self.draw_entry_text.draw_abs(
                cx,
                dvec2(
                    row.pos.x + self.item_pad_x,
                    row.pos.y + (row.size.y - label_size.y) * 0.5,
                ),
                &entry.label,
            );

            if let Some(text) = &entry.shortcut {
                let shown = shortcut_display(text);
                self.draw_shortcut_text.disabled = disabled;
                let size = measure(&self.draw_shortcut_text, cx, &shown);
                self.draw_shortcut_text.draw_abs(
                    cx,
                    dvec2(
                        row.pos.x + row.size.x - self.item_pad_x - size.x,
                        row.pos.y + (row.size.y - size.y) * 0.5,
                    ),
                    &shown,
                );
            }

            self.entry_rects.push(row);
            y += self.item_height;
        }

        cx.end_pass_sized_turtle();
        list.end(cx);
        self.overlay_list = Some(list);
    }

    fn handle_panel(&mut self, cx: &mut Cx, event: &Event) {
        match event.hits(cx, self.draw_panel.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.entry_at(fe.abs);
                cx.set_cursor(if hot.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
                if hot != self.hot_entry {
                    self.hot_entry = hot;
                    self.redraw_overlay(cx);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hot_entry.take().is_some() {
                    self.redraw_overlay(cx);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(fe) if fe.is_primary_hit() && fe.is_over => {
                // `entry_at` already filtered separators and disabled rows.
                let fired = self
                    .entry_at(fe.abs)
                    .and_then(|i| self.defs.get(self.open_menu?)?.items.get(i))
                    .map(|entry| entry.id);
                if let Some(id) = fired {
                    self.close(cx);
                    self.fire(cx, id);
                }
            }
            _ => {}
        }
    }

    fn handle_bar(&mut self, cx: &mut Cx, event: &Event) {
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let hot = self.title_at(fe.abs);
                cx.set_cursor(if hot.is_some() {
                    MouseCursor::Hand
                } else {
                    MouseCursor::Default
                });
                if hot != self.hot_title {
                    self.hot_title = hot;
                    self.draw_bg.redraw(cx);
                }
                // While a menu is open, sliding along the bar switches menus.
                if let (Some(index), Some(open)) = (hot, self.open_menu) {
                    if index != open {
                        self.open(cx, index);
                    }
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hot_title.take().is_some() {
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                if let Some(index) = self.title_at(fe.abs) {
                    if self.open_menu == Some(index) {
                        self.close(cx);
                    } else {
                        self.open(cx, index);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Widget for MenuBar {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let mut walk = walk;
        if !matches!(walk.height, Size::Fixed(_)) {
            walk.height = Size::Fixed(self.bar_height);
        }
        let rect = cx.walk_turtle(walk);
        self.bar_rect = rect;
        self.draw_bg.draw_abs(cx, rect);
        self.draw_titles(cx, rect);

        if let Some(index) = self.open_menu {
            self.draw_menu(cx, index);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::KeyDown(ke) = event {
            if !ke.is_repeat && self.handle_open_key(cx, ke) {
                return;
            }
        }

        if matches!(event, Event::WindowLostFocus(_) | Event::PopupDismissed(_)) {
            self.close(cx);
        }
        if let Event::KeyFocusLost(event) = event {
            if event.prev == self.draw_bg.area() {
                self.close(cx);
            }
        }

        if self.open_menu.is_some() {
            if let Event::MouseDown(me) = event {
                if !self.panel_rect.contains(me.abs) && !self.bar_rect.contains(me.abs) {
                    self.close(cx);
                    // Not a `return`: the press still belongs to whatever
                    // sits underneath it.
                }
            }
        }
        if self.open_menu.is_some() {
            self.handle_panel(cx, event);
        }
        self.handle_bar(cx, event);
    }
}

impl MenuBarRef {
    /// The entry that fired this frame, by click or by keyboard shortcut.
    pub fn selected(&self, actions: &Actions) -> Option<LiveId> {
        // The bar publishes `Closed` in the same frame as `Selected` (an
        // entry closes the menu as it fires), so the first action from the
        // bar is not the one that matters: look for the selection itself.
        let uid = self.widget_uid();
        actions
            .iter()
            .filter_map(|action| action.as_widget_action())
            .filter(|action| action.widget_uid == uid)
            .find_map(|action| match action.cast::<MenuBarAction>() {
                MenuBarAction::Selected(id) => Some(id),
                _ => None,
            })
    }

    pub fn set_menus(&self, cx: &mut Cx, menus: Vec<MenuDef>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_menus(cx, menus);
        }
    }

    pub fn set_enabled(&self, cx: &mut Cx, id: LiveId, enabled: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_enabled(cx, id, enabled);
        }
    }

    pub fn close(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.close(cx);
        }
    }

    pub fn is_open(&self) -> bool {
        self.borrow().map_or(false, |inner| inner.is_open())
    }

    pub fn handle_shortcut(&self, cx: &mut Cx, event: &Event, text_editing: bool) -> bool {
        self.borrow_mut()
            .is_some_and(|mut inner| inner.handle_shortcut(cx, event, text_editing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_script::script;

    fn bare_bar(cx: &mut Cx) -> MenuBar {
        cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                MenuBar{}
            });
            MenuBar::script_from_value(vm, value)
        })
    }

    /// The DSL only fails at eval time, so the gate is a real registration:
    /// build the widget from `mod.widgets.MenuBar` with the menu tree the
    /// host writes, and read back what `on_after_apply` parsed.
    #[test]
    fn the_dsl_registers_and_the_menu_tree_parses() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let bar = cx.with_vm(|vm| {
            let block = script! {
                use mod.prelude.widgets.*
                MenuBar{
                    menus: [
                        {label: "File" items: [
                            {id: @new_from_template label: "New from template" shortcut: "Cmd+N"}
                            {id: @open_flow label: "Open flow" shortcut: "Cmd+O"}
                            {sep: true}
                            {id: @quit label: "Quit" shortcut: "Cmd+Q" enabled: false}
                        ]}
                        {label: "Edit" items: [
                            {id: @undo label: "Undo" shortcut: "Cmd+Z"}
                            {id: @redo label: "Redo" shortcut: "Shift+Cmd+Z"}
                        ]}
                    ]
                }
            };
            let value = vm.eval(block);
            MenuBar::script_from_value(vm, value)
        });

        assert_eq!(bar.defs.len(), 2);
        assert_eq!(bar.defs[0].label, "File");
        assert_eq!(bar.defs[0].items.len(), 4);
        assert_eq!(bar.defs[0].items[0].id, live_id!(new_from_template));
        assert_eq!(bar.defs[0].items[0].label, "New from template");
        assert_eq!(bar.defs[0].items[0].shortcut.as_deref(), Some("Cmd+N"));
        assert!(bar.defs[0].items[0].enabled);
        assert!(bar.defs[0].items[2].separator);
        assert!(!bar.defs[0].items[3].enabled);
        assert_eq!(bar.defs[1].label, "Edit");
        assert_eq!(bar.defs[1].items.len(), 2);
        assert_eq!(bar.defs[1].items[1].shortcut.as_deref(), Some("Shift+Cmd+Z"));
    }

    #[test]
    fn parse_shortcut_reads_a_modifier_chain() {
        let shortcut = parse_shortcut("Shift+Cmd+Z").expect("Shift+Cmd+Z parses");
        assert!(shortcut.cmd);
        assert!(shortcut.shift);
        assert!(!shortcut.alt);
        assert!(!shortcut.ctrl);
        assert_eq!(shortcut.key, KeyCode::KeyZ);
    }

    #[test]
    fn parse_shortcut_reads_bare_and_named_keys() {
        assert_eq!(parse_shortcut("Delete").unwrap().key, KeyCode::Delete);
        assert_eq!(parse_shortcut("Home").unwrap().key, KeyCode::Home);
        assert_eq!(parse_shortcut("F1").unwrap().key, KeyCode::F1);
        assert_eq!(parse_shortcut("Cmd+0").unwrap().key, KeyCode::Key0);
        assert_eq!(parse_shortcut("Cmd+Minus").unwrap().key, KeyCode::Minus);
        let plus = parse_shortcut("Cmd+Plus").unwrap();
        assert_eq!(plus.key, KeyCode::Equals);
        assert!(plus.shift_any);
    }

    #[test]
    fn parse_shortcut_rejects_what_it_cannot_fire() {
        assert!(parse_shortcut("Cmd+Frobnicate").is_none());
        assert!(parse_shortcut("Cmd").is_none());
        assert!(parse_shortcut("").is_none());
        assert!(parse_shortcut("Cmd+N+O").is_none());
    }

    #[test]
    fn shortcut_display_maps_the_modifiers() {
        assert_eq!(shortcut_display("Shift+Cmd+Z"), "\u{21e7}\u{2318}Z");
        assert_eq!(shortcut_display("Cmd+N"), "\u{2318}N");
        assert_eq!(shortcut_display("Ctrl+Alt+Delete"), "\u{2303}\u{2325}Delete");
        assert_eq!(shortcut_display("Cmd+Plus"), "\u{2318}+");
        assert_eq!(shortcut_display("F1"), "F1");
    }

    #[test]
    fn keyboard_navigation_skips_disabled_entries_fires_and_closes_on_blur() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut disabled = MenuEntry::item(live_id!(disabled), "Disabled", None);
        disabled.enabled = false;
        let mut bar = bare_bar(&mut cx);
        bar.defs = vec![
            MenuDef::new(
                "File",
                vec![
                    disabled,
                    MenuEntry::separator(),
                    MenuEntry::item(live_id!(open), "Open", Some("Cmd+O")),
                    MenuEntry::item(live_id!(save), "Save", Some("Cmd+S")),
                ],
            ),
            MenuDef::new(
                "Edit",
                vec![MenuEntry::item(live_id!(undo), "Undo", Some("Cmd+Z"))],
            ),
        ];
        bar.open(&mut cx, 0);
        assert_eq!(bar.hot_entry, Some(2));
        assert!(bar.handle_open_key(
            &mut cx,
            &KeyEvent {
                key_code: KeyCode::ArrowDown,
                ..Default::default()
            }
        ));
        assert_eq!(bar.hot_entry, Some(3));
        bar.handle_open_key(
            &mut cx,
            &KeyEvent {
                key_code: KeyCode::ArrowRight,
                ..Default::default()
            },
        );
        assert_eq!(bar.open_menu, Some(1));
        assert_eq!(bar.hot_entry, Some(0));
        bar.handle_open_key(
            &mut cx,
            &KeyEvent {
                key_code: KeyCode::ReturnKey,
                ..Default::default()
            },
        );
        assert!(!bar.is_open());

        bar.open(&mut cx, 0);
        bar.handle_event(
            &mut cx,
            &Event::WindowLostFocus(WindowId(1, 1)),
            &mut Scope::empty(),
        );
        assert!(!bar.is_open());
    }

    #[test]
    fn text_editor_focus_has_shortcut_precedence() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut bar = bare_bar(&mut cx);
        bar.defs = vec![MenuDef::new(
            "Edit",
            vec![MenuEntry::item(live_id!(undo), "Undo", Some("Cmd+Z"))],
        )];
        let event = Event::KeyDown(KeyEvent {
            key_code: KeyCode::KeyZ,
            modifiers: KeyModifiers {
                logo: true,
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(!bar.handle_shortcut(&mut cx, &event, true));
        assert!(bar.handle_shortcut(&mut cx, &event, false));

        bar.open(&mut cx, 0);
        bar.handle_open_key(
            &mut cx,
            &KeyEvent {
                key_code: KeyCode::Escape,
                ..Default::default()
            },
        );
        assert!(!bar.is_open());
    }
}
