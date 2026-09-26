//! KeyboardMap — the keyboard drawn whole, with what each key does on it.
//!
//! A list of shortcuts answers "what is the chord for this command". The map
//! answers the question the other way round — "what does this key do", and
//! "which keys are still free" — which is the question somebody choosing a
//! new binding is actually asking.
//!
//! The keys come from data tables: the main block in its ANSI or ISO shape
//! (`iso: true`), the function row, the navigation cluster and the number
//! pad, each of which can be left out. Widths are in key units, the way
//! keyboard makers write them, and one unit is `unit` points across.
//!
//! # What the colours say
//!
//! * A key bound in the app's [`Hotkeys`] registry is tinted by the
//!   modifiers of its binding, and carries one small bar per distinct
//!   modifier set bound on it, so `S` bound both bare and with `Mod` shows
//!   two bars. The tints are properties (`tint_plain`, `tint_ctrl`, ...), and
//!   a set of several modifiers mixes theirs.
//! * Holding modifiers narrows the map to the bindings made with exactly
//!   those modifiers, so holding Mod shows every Mod chord at a glance.
//! * The keys held down light up, from the key events the window receives.
//! * The key under the pointer lights up, and its bindings come up as a
//!   tip through the window's tip layer.
//! * A host can light a chord of its own with [`KeyboardMap::set_highlight`]
//!   — a hotkey editor lighting the chord of the row under the pointer.
//!
//! Pressing a key raises [`KeyboardMapAction::KeyClicked`]; what that means
//! (filter a list, start a binding) is the host's. A key press anywhere in
//! the window that the registry resolves to a command raises
//! [`KeyboardMapAction::Resolved`], so the routing can be watched at work.
use crate::{
    badge::{measure, sized},
    hotkeys::{is_modifier_key, Hotkey, HotkeyScope, Hotkeys, KeyChord, KeyPlatform},
    makepad_derive_widget::*,
    makepad_draw::*,
    tip::{TipAction, TipPlace, TipRequest},
    badge::BadgeIntent,
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawKeyboardKey::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.KeyboardMapBase = #(KeyboardMap::register_widget(vm))

    /** A whole keyboard, with the keys the app binds tinted by their
     * modifiers, the held keys lit and the bindings of a hovered key in a
     * tip. */
    mod.widgets.KeyboardMap = set_type_default() do mod.widgets.KeyboardMapBase{
        width: Fit
        height: Fit
        /** default bindings, as on HotkeyEditor */
        hotkeys: []
        /** the ISO main block: the tall Enter and the extra key by left Shift */
        iso: false
        /** the F-key row above the main block */
        show_function_row: true
        /** Insert/Home/PageUp, Delete/End/PageDown and the arrows */
        show_navigation: true
        /** the number pad */
        show_numpad: false
        /** the colour legend under the keys */
        show_legend: true
        /** show only the bindings made with the modifiers held down */
        follow_modifiers: true
        /** one key unit across, in points 16..64 step 1 */
        unit: 34.
        /** the room between two keys 0..8 step 0.5 */
        gap: 3.
        /** the room inside the panel edge 0..40 step 1 */
        pad: 10.
        /** the height of the legend row 12..40 step 1 */
        legend_height: 22.

        tint_plain: theme.color_info
        tint_ctrl: theme.color_primary
        tint_alt: theme.color_warning
        tint_shift: theme.color_success
        tint_logo: theme.color_secondary

        draw_bg +: {
            radius: uniform(theme.radius_m)
            border_size: uniform(1.0)
            color: uniform(theme.color_surface_container)
            border_color: uniform(theme.color_outline_variant)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                sdf.fill_keep(self.color)
                sdf.stroke(self.border_color, self.border_size)
                return sdf.result
            }
        }
        draw_key +: {
            radius: uniform(theme.radius_s)
            border_size: uniform(1.0)
            /** the face of an unbound key */
            color: uniform(theme.color_opaque_u_2)
            /** how strongly a binding tints the face 0..1 step 0.05 */
            tint_strength: uniform(0.45)
            color_hover: uniform(theme.color_opaque_u_6)
            color_held: uniform(theme.color_primary)
            color_marked: uniform(theme.color_primary_container)
            /** a key with no code the app can bind */
            color_inert: uniform(theme.color_surface_container_low)
            border_color: uniform(theme.color_outline_variant)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, self.radius)
                // An ISO Enter is one key over two rows with its lower
                // left corner cut away; every other key has no notch and
                // this cuts a box that lies wholly outside it.
                sdf.box(-1.0, h - self.notch.y, self.notch.x + 1.0, self.notch.y + 1.0, 0.0)
                sdf.subtract()
                let face = mix(self.color, self.tint, self.tint_amount * self.tint_strength)
                let face2 = mix(face, self.color_marked, self.marked)
                let face3 = mix(face2, self.color_hover, self.hover * 0.6)
                let face4 = mix(face3, self.color_held, self.held * 0.8)
                let face5 = mix(face4, self.color_inert, self.inert)
                sdf.fill_keep(face5)
                sdf.stroke(self.border_color, self.border_size)
                return sdf.result
            }
        }
        draw_mark +: {
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 1.0)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_label +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_label_small +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p * 0.8, line_spacing: 1.0}
        }
        draw_legend +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p * 0.9, line_spacing: 1.0}
        }
    }
}

/// One key in a layout table: its code, its width in units, how many rows
/// it spans, the width of the notch cut from its lower left (the ISO Enter),
/// and a label for a key the platform has no code for.
#[derive(Clone, Copy)]
struct KeySpec {
    code: KeyCode,
    width: f64,
    rows: u8,
    notch: f64,
    label: &'static str,
    spacer: bool,
}

const fn key(code: KeyCode) -> KeySpec {
    wide(code, 1.0)
}

const fn wide(code: KeyCode, width: f64) -> KeySpec {
    KeySpec {
        code,
        width,
        rows: 1,
        notch: 0.0,
        label: "",
        spacer: false,
    }
}

const fn tall(code: KeyCode, width: f64, notch: f64) -> KeySpec {
    KeySpec {
        code,
        width,
        rows: 2,
        notch,
        label: "",
        spacer: false,
    }
}

/// A key the platform reports no code for, drawn so the board looks right.
const fn inert(width: f64, label: &'static str) -> KeySpec {
    KeySpec {
        code: KeyCode::Unknown,
        width,
        rows: 1,
        notch: 0.0,
        label,
        spacer: false,
    }
}

const fn space(width: f64) -> KeySpec {
    KeySpec {
        code: KeyCode::Unknown,
        width,
        rows: 1,
        notch: 0.0,
        label: "",
        spacer: true,
    }
}

use KeyCode as K;

const FN_ROW: &[KeySpec] = &[
    key(K::Escape),
    space(1.0),
    key(K::F1),
    key(K::F2),
    key(K::F3),
    key(K::F4),
    space(0.5),
    key(K::F5),
    key(K::F6),
    key(K::F7),
    key(K::F8),
    space(0.5),
    key(K::F9),
    key(K::F10),
    key(K::F11),
    key(K::F12),
];

const NUMBER_ROW: &[KeySpec] = &[
    key(K::Backtick),
    key(K::Key1),
    key(K::Key2),
    key(K::Key3),
    key(K::Key4),
    key(K::Key5),
    key(K::Key6),
    key(K::Key7),
    key(K::Key8),
    key(K::Key9),
    key(K::Key0),
    key(K::Minus),
    key(K::Equals),
    wide(K::Backspace, 2.0),
];

const ANSI_TOP: &[KeySpec] = &[
    wide(K::Tab, 1.5),
    key(K::KeyQ),
    key(K::KeyW),
    key(K::KeyE),
    key(K::KeyR),
    key(K::KeyT),
    key(K::KeyY),
    key(K::KeyU),
    key(K::KeyI),
    key(K::KeyO),
    key(K::KeyP),
    key(K::LBracket),
    key(K::RBracket),
    wide(K::Backslash, 1.5),
];

const ANSI_HOME: &[KeySpec] = &[
    wide(K::Capslock, 1.75),
    key(K::KeyA),
    key(K::KeyS),
    key(K::KeyD),
    key(K::KeyF),
    key(K::KeyG),
    key(K::KeyH),
    key(K::KeyJ),
    key(K::KeyK),
    key(K::KeyL),
    key(K::Semicolon),
    key(K::Quote),
    wide(K::ReturnKey, 2.25),
];

const ANSI_BOTTOM: &[KeySpec] = &[
    wide(K::Shift, 2.25),
    key(K::KeyZ),
    key(K::KeyX),
    key(K::KeyC),
    key(K::KeyV),
    key(K::KeyB),
    key(K::KeyN),
    key(K::KeyM),
    key(K::Comma),
    key(K::Period),
    key(K::Slash),
    wide(K::Shift, 2.75),
];

const ISO_TOP: &[KeySpec] = &[
    wide(K::Tab, 1.5),
    key(K::KeyQ),
    key(K::KeyW),
    key(K::KeyE),
    key(K::KeyR),
    key(K::KeyT),
    key(K::KeyY),
    key(K::KeyU),
    key(K::KeyI),
    key(K::KeyO),
    key(K::KeyP),
    key(K::LBracket),
    key(K::RBracket),
    // 1.5 across on this row, 1.25 on the next.
    tall(K::ReturnKey, 1.5, 0.25),
];

const ISO_HOME: &[KeySpec] = &[
    wide(K::Capslock, 1.75),
    key(K::KeyA),
    key(K::KeyS),
    key(K::KeyD),
    key(K::KeyF),
    key(K::KeyG),
    key(K::KeyH),
    key(K::KeyJ),
    key(K::KeyK),
    key(K::KeyL),
    key(K::Semicolon),
    key(K::Quote),
    // The key left of the tall Enter reports the backslash code.
    key(K::Backslash),
];

const ISO_BOTTOM: &[KeySpec] = &[
    wide(K::Shift, 1.25),
    inert(1.0, "\\"),
    key(K::KeyZ),
    key(K::KeyX),
    key(K::KeyC),
    key(K::KeyV),
    key(K::KeyB),
    key(K::KeyN),
    key(K::KeyM),
    key(K::Comma),
    key(K::Period),
    key(K::Slash),
    wide(K::Shift, 2.75),
];

const SPACE_ROW: &[KeySpec] = &[
    wide(K::Control, 1.25),
    wide(K::Logo, 1.25),
    wide(K::Alt, 1.25),
    wide(K::Space, 6.25),
    wide(K::Alt, 1.25),
    wide(K::Logo, 1.25),
    inert(1.25, "Menu"),
    wide(K::Control, 1.25),
];

const SPACE_ROW_APPLE: &[KeySpec] = &[
    wide(K::Control, 1.25),
    wide(K::Alt, 1.25),
    wide(K::Logo, 1.5),
    wide(K::Space, 7.0),
    wide(K::Logo, 1.5),
    wide(K::Alt, 1.25),
    wide(K::Control, 1.25),
];

const NAV_FN: &[KeySpec] = &[key(K::PrintScreen), key(K::ScrollLock), key(K::Pause)];
const NAV_1: &[KeySpec] = &[key(K::Insert), key(K::Home), key(K::PageUp)];
const NAV_2: &[KeySpec] = &[key(K::Delete), key(K::End), key(K::PageDown)];
const NAV_4: &[KeySpec] = &[space(1.0), key(K::ArrowUp)];
const NAV_5: &[KeySpec] = &[key(K::ArrowLeft), key(K::ArrowDown), key(K::ArrowRight)];

const PAD_1: &[KeySpec] = &[
    key(K::Numlock),
    key(K::NumpadDivide),
    key(K::NumpadMultiply),
    key(K::NumpadSubtract),
];
const PAD_2: &[KeySpec] = &[
    key(K::Numpad7),
    key(K::Numpad8),
    key(K::Numpad9),
    tall(K::NumpadAdd, 1.0, 0.0),
];
const PAD_3: &[KeySpec] = &[key(K::Numpad4), key(K::Numpad5), key(K::Numpad6)];
const PAD_4: &[KeySpec] = &[
    key(K::Numpad1),
    key(K::Numpad2),
    key(K::Numpad3),
    tall(K::NumpadEnter, 1.0, 0.0),
];
const PAD_5: &[KeySpec] = &[wide(K::Numpad0, 2.0), key(K::NumpadDecimal)];

const EMPTY: &[KeySpec] = &[];

/// The main block is 15 units across; the navigation cluster 3; the pad 4.
const MAIN_UNITS: f64 = 15.0;
const NAV_UNITS: f64 = 3.0;
const PAD_UNITS: f64 = 4.0;
/// The room between the blocks, and under the function row.
const BLOCK_GAP: f64 = 0.5;
const FN_GAP: f64 = 0.25;

/// A block's six rows: the function row, then the five main rows.
type Block = [&'static [KeySpec]; 6];

fn main_block(iso: bool, platform: KeyPlatform) -> Block {
    let space_row = match platform {
        KeyPlatform::Apple => SPACE_ROW_APPLE,
        KeyPlatform::Other => SPACE_ROW,
    };
    if iso {
        [FN_ROW, NUMBER_ROW, ISO_TOP, ISO_HOME, ISO_BOTTOM, space_row]
    } else {
        [FN_ROW, NUMBER_ROW, ANSI_TOP, ANSI_HOME, ANSI_BOTTOM, space_row]
    }
}

const NAV_BLOCK: Block = [NAV_FN, NAV_1, NAV_2, EMPTY, NAV_4, NAV_5];
const PAD_BLOCK: Block = [EMPTY, PAD_1, PAD_2, PAD_3, PAD_4, PAD_5];

/// What a key cap says on this platform.
pub fn key_cap_label(code: KeyCode, platform: KeyPlatform) -> String {
    let apple = platform == KeyPlatform::Apple;
    let text = match code {
        K::Escape => "Esc",
        K::Backspace => {
            if apple {
                "Delete"
            } else {
                "Backspace"
            }
        }
        K::Tab => "Tab",
        K::Capslock => "Caps",
        K::ReturnKey => {
            if apple {
                "Return"
            } else {
                "Enter"
            }
        }
        K::Shift => {
            if apple {
                "\u{21e7}"
            } else {
                "Shift"
            }
        }
        K::Control => {
            if apple {
                "\u{2303}"
            } else {
                "Ctrl"
            }
        }
        K::Alt => {
            if apple {
                "\u{2325}"
            } else {
                "Alt"
            }
        }
        K::Logo => {
            if apple {
                "\u{2318}"
            } else if cfg!(target_os = "windows") {
                "Win"
            } else {
                "Super"
            }
        }
        K::Space => "",
        K::Delete => "Del",
        K::Insert => "Ins",
        K::PageUp => "PgUp",
        K::PageDown => "PgDn",
        K::ArrowUp => "\u{2191}",
        K::ArrowDown => "\u{2193}",
        K::ArrowLeft => "\u{2190}",
        K::ArrowRight => "\u{2192}",
        K::PrintScreen => "PrtSc",
        K::ScrollLock => "ScrLk",
        K::Numlock => "Num",
        K::Numpad0 => "0",
        K::Numpad1 => "1",
        K::Numpad2 => "2",
        K::Numpad3 => "3",
        K::Numpad4 => "4",
        K::Numpad5 => "5",
        K::Numpad6 => "6",
        K::Numpad7 => "7",
        K::Numpad8 => "8",
        K::Numpad9 => "9",
        K::NumpadDivide => "/",
        K::NumpadMultiply => "*",
        K::NumpadSubtract => "-",
        K::NumpadAdd => "+",
        K::NumpadDecimal => ".",
        K::NumpadEnter => "Enter",
        other => crate::hotkeys::key_name(other).unwrap_or(""),
    };
    text.to_string()
}

/// One key as drawn.
#[derive(Clone, Debug)]
struct PlacedKey {
    code: KeyCode,
    rect: Rect,
    /// The notch cut from the lower left, in points.
    notch: DVec2,
    label: String,
}

/// The key's face and state.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawKeyboardKey {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    tint: Vec4f,
    #[live]
    tint_amount: f32,
    #[live]
    hover: f32,
    #[live]
    held: f32,
    #[live]
    marked: f32,
    #[live]
    inert: f32,
    #[live]
    notch: Vec2f,
}

#[derive(Clone, Debug, Default)]
pub enum KeyboardMapAction {
    /// A key with a code was pressed with the pointer.
    KeyClicked(KeyCode),
    /// A key press in the window resolved to this command in the registry,
    /// by the registry's routing rules. Reported so a host can show the
    /// routing at work; running the command is still the host's job.
    Resolved(LiveId),
    #[default]
    None,
}

#[derive(Script, Widget)]
pub struct KeyboardMap {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_key: DrawKeyboardKey,
    #[live]
    draw_mark: DrawColor,
    #[live]
    draw_label: DrawText,
    #[live]
    draw_label_small: DrawText,
    #[live]
    draw_legend: DrawText,

    /// Default bindings written in the DSL, registered on apply.
    #[live]
    hotkeys: ScriptValue,

    #[live]
    pub iso: bool,
    #[live(true)]
    pub show_function_row: bool,
    #[live(true)]
    pub show_navigation: bool,
    #[live]
    pub show_numpad: bool,
    #[live(true)]
    pub show_legend: bool,
    #[live(true)]
    pub follow_modifiers: bool,
    #[live(34.0)]
    pub unit: f64,
    #[live(3.0)]
    pub gap: f64,
    #[live(10.0)]
    pub pad: f64,
    #[live(22.0)]
    pub legend_height: f64,

    #[live]
    pub tint_plain: Vec4f,
    #[live]
    pub tint_ctrl: Vec4f,
    #[live]
    pub tint_alt: Vec4f,
    #[live]
    pub tint_shift: Vec4f,
    #[live]
    pub tint_logo: Vec4f,

    #[live(true)]
    #[visible]
    visible: bool,

    #[rust]
    hotkeys_source: ScriptValue,
    #[rust]
    keys: Vec<PlacedKey>,
    #[rust]
    hover: Option<usize>,
    /// The key whose tip is up.
    #[rust]
    tip_for: Option<usize>,
    /// Keys held down, from the window's key events.
    #[rust]
    held: Vec<KeyCode>,
    /// A chord a host asked to light.
    #[rust]
    highlight: Option<KeyChord>,
    #[rust]
    drawn_revision: u64,
}

impl ScriptHook for KeyboardMap {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if self.hotkeys.raw() != self.hotkeys_source.raw() {
            self.hotkeys_source = self.hotkeys;
            Hotkeys::register_script(vm, self.hotkeys);
        }
        let cx = vm.cx_mut();
        self.draw_bg.redraw(cx);
    }
}

impl KeyboardMap {
    /// Drop a held modifier the latest key event says is no longer down, so
    /// a modifier released while the window was not looking does not stay
    /// lit. `pressed` is the key the event is about, which counts as down.
    fn sync_modifiers(&mut self, m: KeyModifiers, pressed: KeyCode) -> bool {
        let before = self.held.len();
        self.held.retain(|code| {
            *code == pressed
                || match code {
                    KeyCode::Control => m.control,
                    KeyCode::Alt => m.alt,
                    KeyCode::Shift => m.shift,
                    KeyCode::Logo => m.logo,
                    _ => true,
                }
        });
        self.held.len() != before
    }

    /// Light a chord's key and its modifier keys, or nothing with `None`.
    pub fn set_highlight(&mut self, cx: &mut Cx, chord: Option<KeyChord>) {
        if self.highlight != chord {
            self.highlight = chord;
            self.redraw(cx);
        }
    }

    /// The modifier set held down, when `follow_modifiers` is on and any
    /// modifier is down.
    fn held_modifiers(&self) -> Option<KeyModifiers> {
        if !self.follow_modifiers {
            return None;
        }
        let m = KeyModifiers {
            shift: self.held.contains(&KeyCode::Shift),
            control: self.held.contains(&KeyCode::Control),
            alt: self.held.contains(&KeyCode::Alt),
            logo: self.held.contains(&KeyCode::Logo),
        };
        m.any().then_some(m)
    }

    /// The bindings a key shows: all of them, or only those with the held
    /// modifiers.
    fn shown_bindings<'a>(&self, hotkeys: &'a Hotkeys, code: KeyCode) -> Vec<&'a Hotkey> {
        let filter = self.held_modifiers();
        hotkeys
            .bindings_on_key(code)
            .into_iter()
            .filter(|h| match (filter, h.chord) {
                (Some(m), Some(chord)) => chord.modifiers == m,
                _ => true,
            })
            .collect()
    }

    /// The tint of a modifier set: the mean of its modifiers' tints, or the
    /// plain tint for none.
    fn tint_for(&self, m: KeyModifiers) -> Vec4f {
        let mut parts: Vec<Vec4f> = Vec::new();
        if m.control {
            parts.push(self.tint_ctrl);
        }
        if m.alt {
            parts.push(self.tint_alt);
        }
        if m.shift {
            parts.push(self.tint_shift);
        }
        if m.logo {
            parts.push(self.tint_logo);
        }
        if parts.is_empty() {
            return self.tint_plain;
        }
        let n = parts.len() as f32;
        let mut out = Vec4f::default();
        for p in parts {
            out.x += p.x / n;
            out.y += p.y / n;
            out.z += p.z / n;
            out.w += p.w / n;
        }
        out
    }

    /// Whether a key is lit by the host's highlight: its key, or one of its
    /// modifiers.
    fn is_marked(&self, code: KeyCode) -> bool {
        let Some(chord) = self.highlight else {
            return false;
        };
        let m = chord.modifiers;
        code == chord.key
            || (code == KeyCode::Control && m.control)
            || (code == KeyCode::Alt && m.alt)
            || (code == KeyCode::Shift && m.shift)
            || (code == KeyCode::Logo && m.logo)
    }

    /// The size of the board in units, and where each block starts.
    fn blocks(&self, platform: KeyPlatform) -> (DVec2, Vec<(f64, Block)>) {
        let mut blocks = vec![(0.0, main_block(self.iso, platform))];
        let mut width = MAIN_UNITS;
        if self.show_navigation {
            blocks.push((width + BLOCK_GAP, NAV_BLOCK));
            width += BLOCK_GAP + NAV_UNITS;
        }
        if self.show_numpad {
            blocks.push((width + BLOCK_GAP, PAD_BLOCK));
            width += BLOCK_GAP + PAD_UNITS;
        }
        let height = 5.0
            + if self.show_function_row {
                1.0 + FN_GAP
            } else {
                0.0
            };
        (dvec2(width, height), blocks)
    }

    /// Lay every key out from the tables, `origin` at the top left.
    fn place_keys(&mut self, origin: DVec2, platform: KeyPlatform) {
        let (_, blocks) = self.blocks(platform);
        let unit = self.unit;
        self.keys.clear();
        for (block_x, rows) in blocks {
            for (row_index, row) in rows.iter().enumerate() {
                if row_index == 0 && !self.show_function_row {
                    continue;
                }
                let row_y = if row_index == 0 {
                    0.0
                } else if self.show_function_row {
                    1.0 + FN_GAP + (row_index - 1) as f64
                } else {
                    (row_index - 1) as f64
                };
                let mut x = block_x;
                for spec in row.iter() {
                    if !spec.spacer {
                        let rect = Rect {
                            pos: dvec2(
                                origin.x + x * unit + self.gap * 0.5,
                                origin.y + row_y * unit + self.gap * 0.5,
                            ),
                            size: dvec2(
                                spec.width * unit - self.gap,
                                spec.rows as f64 * unit - self.gap,
                            ),
                        };
                        let notch = if spec.notch > 0.0 {
                            dvec2(spec.notch * unit, unit)
                        } else {
                            DVec2::default()
                        };
                        let label = if spec.label.is_empty() {
                            key_cap_label(spec.code, platform)
                        } else {
                            spec.label.to_string()
                        };
                        self.keys.push(PlacedKey {
                            code: spec.code,
                            rect,
                            notch,
                            label,
                        });
                    }
                    x += spec.width;
                }
            }
        }
    }

    fn key_at(&self, abs: DVec2) -> Option<usize> {
        self.keys.iter().position(|key| {
            if !key.rect.contains(abs) {
                return false;
            }
            // Outside the notch of an ISO Enter.
            let in_notch = abs.x < key.rect.pos.x + key.notch.x
                && abs.y > key.rect.pos.y + key.rect.size.y - key.notch.y;
            !in_notch
        })
    }

    /// What the tip over a key says: one line per binding shown on it.
    fn tip_text(&self, hotkeys: &Hotkeys, code: KeyCode) -> String {
        let lines: Vec<String> = self
            .shown_bindings(hotkeys, code)
            .into_iter()
            .map(|h| {
                let chord = h.chord.map(|c| c.format()).unwrap_or_default();
                match h.scope {
                    HotkeyScope::Global => format!("{chord}  {}", h.label),
                    HotkeyScope::Focused(scope) => format!("{chord}  {} (in {scope})", h.label),
                }
            })
            .collect();
        lines.join("\n")
    }

    fn set_hover(&mut self, cx: &mut Cx, hover: Option<usize>) {
        if hover == self.hover {
            return;
        }
        self.hover = hover;
        let text = match hover.and_then(|index| self.keys.get(index)) {
            Some(key) if !key.code.is_unknown() => {
                let code = key.code;
                let hotkeys = cx.global::<Hotkeys>();
                self.tip_text(hotkeys, code)
            }
            _ => String::new(),
        };
        if text.is_empty() {
            if self.tip_for.take().is_some() {
                cx.widget_action(self.uid, TipAction::HoverOut);
            }
        } else if let Some(index) = hover {
            self.tip_for = Some(index);
            let request = TipRequest {
                text,
                anchor: self.keys[index].rect,
                place: TipPlace::Top,
                arrow: true,
                wrap_width: 0.0,
                delay_secs: -1.0,
                intent: BadgeIntent::Neutral,
            };
            cx.widget_action(self.uid, TipAction::HoverInWith(request));
        }
        cx.set_cursor(if hover.is_some() {
            MouseCursor::Hand
        } else {
            MouseCursor::Default
        });
        self.redraw(cx);
    }

    fn draw_legend_row(&mut self, cx: &mut Cx2d, left: f64, y: f64) {
        let apple = KeyPlatform::current() == KeyPlatform::Apple;
        let entries: [(&str, KeyModifiers); 5] = [
            ("Plain", KeyModifiers::default()),
            (
                if apple { "\u{2303} Ctrl" } else { "Ctrl" },
                KeyModifiers {
                    control: true,
                    ..Default::default()
                },
            ),
            (
                if apple { "\u{2325} Opt" } else { "Alt" },
                KeyModifiers {
                    alt: true,
                    ..Default::default()
                },
            ),
            (
                if apple { "\u{21e7} Shift" } else { "Shift" },
                KeyModifiers {
                    shift: true,
                    ..Default::default()
                },
            ),
            (
                if apple { "\u{2318} Cmd" } else { "Super" },
                KeyModifiers {
                    logo: true,
                    ..Default::default()
                },
            ),
        ];
        let row = Rect {
            pos: dvec2(left, y),
            size: dvec2(0.0, self.legend_height),
        };
        let fs = self.draw_legend.text_style.font_size as f64;
        let ty = row.pos.y + (row.size.y - fs) * 0.5 - fs * 0.3;
        let swatch = 10.0;
        let mut x = left;
        for (label, m) in entries {
            self.draw_mark.color = self.tint_for(m);
            self.draw_mark.draw_abs(
                cx,
                Rect {
                    pos: dvec2(x, row.pos.y + (row.size.y - swatch) * 0.5),
                    size: dvec2(swatch, swatch),
                },
            );
            x += swatch + 5.0;
            self.draw_legend.draw_abs(cx, dvec2(x, ty), label);
            x += measure(&self.draw_legend, cx, label) + 14.0;
        }
    }
}

impl Widget for KeyboardMap {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let platform = KeyPlatform::current();
        let (units, _) = self.blocks(platform);
        let legend = if self.show_legend {
            self.legend_height + self.pad * 0.5
        } else {
            0.0
        };
        let size = dvec2(
            units.x * self.unit + self.pad * 2.0,
            units.y * self.unit + self.pad * 2.0 + legend,
        );
        self.draw_bg.begin(cx, sized(walk, size.x, size.y), self.layout);
        let panel = cx.turtle().rect();
        let origin = panel.pos + dvec2(self.pad, self.pad);
        self.place_keys(origin, platform);

        // What each key shows, read once from the registry.
        let bound: Vec<Vec<KeyModifiers>> = {
            let hotkeys = cx.global::<Hotkeys>();
            self.drawn_revision = hotkeys.revision();
            self.keys
                .iter()
                .map(|key| {
                    let mut sets: Vec<KeyModifiers> = Vec::new();
                    if !key.code.is_unknown() && !is_modifier_key(key.code) {
                        for hotkey in self.shown_bindings(hotkeys, key.code) {
                            if let Some(chord) = hotkey.chord {
                                if !sets.contains(&chord.modifiers) {
                                    sets.push(chord.modifiers);
                                }
                            }
                        }
                    }
                    sets
                })
                .collect()
        };

        let unit = self.unit;
        for index in 0..self.keys.len() {
            let key = self.keys[index].clone();
            let sets = &bound[index];
            self.draw_key.tint = sets
                .first()
                .map(|m| self.tint_for(*m))
                .unwrap_or_default();
            self.draw_key.tint_amount = if sets.is_empty() { 0.0 } else { 1.0 };
            self.draw_key.hover = if self.hover == Some(index) { 1.0 } else { 0.0 };
            self.draw_key.held = if self.held.contains(&key.code) && !key.code.is_unknown() {
                1.0
            } else {
                0.0
            };
            self.draw_key.marked = if self.is_marked(key.code) { 1.0 } else { 0.0 };
            self.draw_key.inert = if key.code.is_unknown() { 1.0 } else { 0.0 };
            self.draw_key.notch = Vec2f {
                x: key.notch.x as f32,
                y: key.notch.y as f32,
            };
            self.draw_key.draw_abs(cx, key.rect);

            // The label sits in the top unit of a tall key, so the ISO
            // Enter's reads on the row it starts on.
            let face = Rect {
                pos: key.rect.pos,
                size: dvec2(key.rect.size.x, if key.notch.x > 0.0 { unit - self.gap } else { key.rect.size.y }),
            };
            if !key.label.is_empty() {
                let small = key.label.chars().count() > 2;
                let text = if small {
                    &mut self.draw_label_small
                } else {
                    &mut self.draw_label
                };
                let fs = text.text_style.font_size as f64;
                let width = measure(text, cx, &key.label);
                let pos = dvec2(
                    face.pos.x + ((face.size.x - width) * 0.5).max(2.0),
                    face.pos.y + (face.size.y - fs) * 0.5 - fs * 0.3,
                );
                text.draw_abs(cx, pos, &key.label);
            }

            // One bar per modifier set bound on the key.
            if !sets.is_empty() {
                let count = sets.len().min(4);
                let bar_h = 3.0;
                let inset = 4.0;
                let span = (key.rect.size.x - inset * 2.0).max(4.0);
                let bar_w = (span - (count - 1) as f64 * 2.0) / count as f64;
                for (i, m) in sets.iter().take(count).enumerate() {
                    self.draw_mark.color = self.tint_for(*m);
                    self.draw_mark.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(
                                key.rect.pos.x + inset + i as f64 * (bar_w + 2.0),
                                key.rect.pos.y + key.rect.size.y - bar_h - 3.0,
                            ),
                            size: dvec2(bar_w, bar_h),
                        },
                    );
                }
            }
        }

        if self.show_legend {
            let y = origin.y + units.y * unit + self.pad * 0.5;
            self.draw_legend_row(cx, origin.x, y);
        }

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if cx.global::<Hotkeys>().revision() != self.drawn_revision {
            self.drawn_revision = cx.global::<Hotkeys>().revision();
            self.redraw(cx);
        }
        match event {
            Event::KeyDown(ke) => {
                if !ke.is_repeat {
                    if let Some(id) = Hotkeys::resolve_in(cx, ke) {
                        cx.widget_action(self.uid, KeyboardMapAction::Resolved(id));
                    }
                }
                let mut changed = self.sync_modifiers(ke.modifiers, ke.key_code);
                if !self.held.contains(&ke.key_code) {
                    self.held.push(ke.key_code);
                    changed = true;
                }
                if changed {
                    self.redraw(cx);
                }
            }
            Event::KeyUp(ke) => {
                let before = self.held.clone();
                self.held.retain(|code| *code != ke.key_code);
                // Some platforms send no key-up for a key released while
                // Command is down; letting go of Command lets go of those.
                if ke.key_code == KeyCode::Logo {
                    self.held.retain(|code| is_modifier_key(*code));
                }
                if self.held != before {
                    self.redraw(cx);
                }
            }
            Event::WindowLostFocus(_) => {
                if !self.held.is_empty() {
                    self.held.clear();
                    self.redraw(cx);
                }
            }
            _ => {}
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                let at = self.key_at(fe.abs);
                self.set_hover(cx, at);
            }
            Hit::FingerHoverOut(_) => self.set_hover(cx, None),
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                if self.tip_for.take().is_some() {
                    cx.widget_action(self.uid, TipAction::HoverOut);
                }
                if let Some(index) = self.key_at(fe.abs) {
                    let code = self.keys[index].code;
                    if !code.is_unknown() {
                        cx.widget_action(self.uid, KeyboardMapAction::KeyClicked(code));
                    }
                }
            }
            _ => {}
        }
    }

    /// The keys held down, by name.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        let names: Vec<String> = self
            .held
            .iter()
            .map(|code| key_cap_label(*code, KeyPlatform::Other))
            .collect();
        Some(names.join(" "))
    }
}

impl KeyboardMapRef {
    pub fn set_highlight(&self, cx: &mut Cx, chord: Option<KeyChord>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_highlight(cx, chord);
        }
    }

    /// The command a key press resolved to this pass.
    pub fn resolved(&self, actions: &Actions) -> Option<LiveId> {
        actions
            .filter_widget_actions_cast::<KeyboardMapAction>(self.widget_uid())
            .find_map(|action| match action {
                KeyboardMapAction::Resolved(id) => Some(id),
                _ => None,
            })
    }

    /// The key pressed with the pointer this pass.
    pub fn key_clicked(&self, actions: &Actions) -> Option<KeyCode> {
        actions
            .filter_widget_actions_cast::<KeyboardMapAction>(self.widget_uid())
            .find_map(|action| match action {
                KeyboardMapAction::KeyClicked(code) => Some(code),
                _ => None,
            })
    }
}
