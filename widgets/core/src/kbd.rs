//! Kbd, KbdGroup and ShortcutHelp — what to press, drawn as the keys.
//!
//! A chord written into a sentence ("press ctrl+shift+P") reads as prose and
//! scans as nothing. Drawn as caps it is a small picture of the keyboard,
//! and the eye finds it in a paragraph without reading a word of it. That is
//! the whole of what these three are for.
//!
//! * [`Kbd`] is ONE cap: a glyph or a short word in a small rounded box.
//! * [`KbdGroup`] takes a whole chord as a string — `"ctrl+shift+P"` — and
//!   draws its caps in order with a separator between them.
//! * [`ShortcutHelp`] is a list of chords and what they do, grouped under
//!   headings, with a search field over it.
//!
//! # The modifier names belong to the platform, the shortcut string does not
//!
//! One platform prints its modifiers on the keys as symbols; every other one
//! spells them as words. The difference is not decoration: on the symbol
//! platform the words are not written on any key, and the two vocabularies
//! do not even agree on the order the modifiers are read in. So a call site
//! writes one string, `"ctrl+shift+P"`, and each build turns it into the
//! caps its own keyboard has on it — names and order both, chosen at compile
//! time from the build target's own vendor field.
//!
//! The parser takes the modifiers in any order and prints them in one, so a
//! list of shortcuts written by several hands still reads as one list.
//!
//! # What these deliberately do not do
//!
//! They do not bind anything. A cap is a picture of a key, not a key: no
//! widget here listens for the chord it draws, and nothing fires when one is
//! pressed. Binding belongs with the thing being bound — the menu, the
//! command list — and a widget that both drew and bound would quietly become
//! the place shortcuts are defined, which is the last place to look for one.
//!
//! They also do not know what the host actually bound. Nothing checks that
//! `"ctrl+shift+P"` is a chord the app answers to; a help list is a list of
//! claims, and keeping the claims true is the host's job.
use crate::{
    badge::{measure, sized},
    makepad_derive_widget::*,
    makepad_draw::*,
    text_input::{TextInput, TextInputAction},
    widget::*,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // ShortcutHelp owns a TextInput, so this module has to register after
    // that one: a block's `use` only sees what already exists, and a slot
    // filled from a name that is not there yet is an empty slot.

    set_type_default() do #(DrawKbdCap::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    set_type_default() do #(DrawKbdGround::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            // A chord's own rect, painted as nothing. Every cap is drawn at
            // an absolute position, which would otherwise leave the group no
            // rect of its own to redraw, to hover or to be found by.
            return vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    mod.widgets.KbdBase = #(Kbd::register_widget(vm))

    /** One key cap: a glyph or a short word in a small rounded box. */
    mod.widgets.KbdFlat = set_type_default() do mod.widgets.KbdBase{
        width: Fit
        height: 22.
        /** what is printed on the cap */
        text: "K"
        /** drawn as if the key were held down */
        pressed: false
        /** the least a cap may be wide, so one letter is still a key 8..80 step 1 */
        min_width: 22.
        /** room either side of the text 0..24 step 0.5 */
        pad_x: 7.
        /** the height a cap takes when its walk fixes none 12..48 step 1 */
        cap_height: 22.
        /** the side of the key seen from above; 0 is a flat cap 0..8 step 0.5 */
        shelf: 0.

        draw_bg +: {
            /** corner rounding 0..12 step 0.5 */
            radius: uniform(theme.radius_s)
            /** outline width in pixels 0..3 step 0.5 */
            border_size: uniform(1.0)
            /** the face of the cap */
            color: uniform(theme.color_opaque_u_2)
            /** the face while the key is held down */
            color_pressed: uniform(theme.color_opaque_u_1)
            /** the side of the key, under the face */
            edge_color: uniform(theme.color_opaque_d_1)
            border_color: uniform(theme.color_outline_variant)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let w = self.rect_size.x
                let h = self.rect_size.y
                // The face sits `shelf` short of the bottom of the box and
                // drops onto it when the key is held: the FACE moves and the
                // box does not, so a pressed cap takes exactly the room a
                // resting one takes and a row of caps never twitches.
                let sink = self.shelf * self.pressed
                sdf.box(0.5, 0.5, w - 1.0, h - 1.0, self.radius)
                sdf.fill_keep(self.edge_color)
                sdf.stroke(self.border_color, self.border_size)
                sdf.box(1.5, 1.0 + sink, w - 3.0, h - 2.0 - self.shelf, self.radius)
                sdf.fill(mix(self.color, self.color_pressed, self.pressed))
                return sdf.result
            }
        }
        draw_text +: {
            color: theme.color_text
            // Line spacing of one, so the line box IS the ink box: the glyph
            // is placed against the cap by arithmetic, not by the layouter,
            // and the two only agree when the line has no leading in it.
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
    }

    /** The standard cap: the flat face, plus the side of the key under it. */
    mod.widgets.Kbd = set_type_default() do mod.widgets.KbdFlat{
        shelf: 2.
    }

    mod.widgets.KbdGroupBase = #(KbdGroup::register_widget(vm))

    /** A chord: several caps in the order they are pressed, with a separator
     * between them. The modifier names are the ones this platform prints on
     * its own keys. */
    mod.widgets.KbdGroup = set_type_default() do mod.widgets.KbdGroupBase{
        width: Fit
        height: 22.
        /** the chord, written with `+`: "ctrl+shift+P" */
        shortcut: ""
        /** what goes between two caps; "" for nothing */
        separator: "+"
        /** room either side of a separator 0..20 step 0.5 */
        gap: 3.
        /** the height the caps take when the walk fixes none 12..48 step 1 */
        cap_height: 22.
        // A property, not a named child: the group draws this ONE cap once
        // per key in the chord, so a chord is a handful of instances in one
        // draw call and the cap's look is declared in a single place.
        cap: mod.widgets.Kbd{}
        draw_sep +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
    }

    /** A chord with nothing between the caps, the way a keyboard whose
     * modifiers are symbols normally prints one. */
    mod.widgets.KbdGroupTight = set_type_default() do mod.widgets.KbdGroup{
        separator: ""
        gap: 1.
    }

    mod.widgets.ShortcutHelpBase = #(ShortcutHelp::register_widget(vm))

    /** A searchable list of shortcuts: one row per chord, grouped under
     * headings. Each line of `entries` is either "# Heading" or
     * "What it does = ctrl+c". */
    mod.widgets.ShortcutHelp = set_type_default() do mod.widgets.ShortcutHelpBase{
        width: Fill
        height: Fit
        /** the lines: "# Heading" opens a group, "Label = chord" is a row */
        entries: []
        /** shown in place of the rows when the search matches nothing */
        empty_text: "Nothing matches"
        /** the room inside the panel edge 0..40 step 1 */
        pad: 10.
        /** the height of one shortcut row 16..48 step 1 */
        row_height: 24.
        /** the height of a heading row 16..48 step 1 */
        heading_height: 28.
        /** the height of the search field; 0 leaves it out 0..60 step 1 */
        search_height: 26.
        /** the room under the search field 0..40 step 1 */
        search_gap: 8.
        /** the height of the caps in a row 12..40 step 1 */
        cap_height: 20.
        // Clipped, because a label longer than its row must stop at the
        // panel edge rather than run out over whatever is beside it.
        clip_x: true

        search: mod.widgets.TextInput{
            empty_text: "Search shortcuts"
        }
        chord: mod.widgets.KbdGroup{}

        draw_bg +: {
            /** corner rounding 0..24 step 0.5 */
            radius: uniform(theme.radius_m)
            /** outline width in pixels 0..3 step 0.5 */
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
        draw_row +: {
            /** corner rounding of the band under the pointer 0..16 step 0.5 */
            radius: uniform(theme.radius_s)
            color: uniform(theme.color_opaque_u_1)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
                sdf.fill(self.color)
                return sdf.result
            }
        }
        draw_heading +: {
            color: theme.color_text_meta
            text_style: theme.font_bold{font_size: theme.font_size_p, line_spacing: 1.0}
        }
        draw_label +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: theme.font_size_p, line_spacing: 1.0}
        }
    }
}

/// Where a glyph's ink starts below the y handed to `draw_abs`, as a share
/// of the font size. `draw_abs` takes the top of the LINE box, not the ink;
/// centring the line box leaves a run of capitals and digits riding high,
/// because they have no descender to fill the bottom of the line.
const INK_TOP: f64 = 0.30;

/// What a help panel asks for when nothing has told it how wide to be.
const DEFAULT_HELP_WIDTH: f64 = 320.0;

/// Where one line of text goes so that it sits in the middle of `rect`.
fn ink_y(rect: Rect, font_size: f64) -> f64 {
    rect.pos.y + (rect.size.y - font_size) * 0.5 - font_size * INK_TOP
}

/// A modifier key, whatever name it was written under.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    /// The one whose printed name differs most between platforms.
    Meta,
}

impl Modifier {
    /// The names a shortcut string may use. They are all accepted
    /// everywhere: a string written on one platform has to keep working on
    /// another, or a shared shortcut table would need a copy per build.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "ctrl" | "control" | "ctl" => Modifier::Ctrl,
            "alt" | "option" | "opt" => Modifier::Alt,
            "shift" => Modifier::Shift,
            "cmd" | "command" | "meta" | "super" => Modifier::Meta,
            _ => return None,
        })
    }

    /// What goes on the cap. Where the modifiers are symbols, the symbols
    /// are the only names printed on the keys themselves.
    pub fn cap(self, symbols: bool) -> &'static str {
        match (self, symbols) {
            (Modifier::Ctrl, true) => "\u{2303}",
            (Modifier::Ctrl, false) => "Ctrl",
            (Modifier::Alt, true) => "\u{2325}",
            (Modifier::Alt, false) => "Alt",
            (Modifier::Shift, true) => "\u{21e7}",
            (Modifier::Shift, false) => "Shift",
            (Modifier::Meta, true) => "\u{2318}",
            (Modifier::Meta, false) => "Meta",
        }
    }
}

/// The order the modifiers are printed in, which is not the same in the two
/// vocabularies. Both are conventions people read at a glance, so following
/// the local one matters more than having a single order everywhere.
const SYMBOL_ORDER: [Modifier; 4] =
    [Modifier::Ctrl, Modifier::Alt, Modifier::Shift, Modifier::Meta];
const WORD_ORDER: [Modifier; 4] =
    [Modifier::Ctrl, Modifier::Meta, Modifier::Alt, Modifier::Shift];

/// The keys with a name of their own, and the face each one shows. The
/// arrows are drawn as their glyphs because that is what the keys carry.
const NAMED_KEYS: &[(&str, &str)] = &[
    ("esc", "Esc"),
    ("escape", "Esc"),
    ("tab", "Tab"),
    ("space", "Space"),
    ("spacebar", "Space"),
    ("enter", "Enter"),
    ("return", "Enter"),
    ("backspace", "Backspace"),
    ("delete", "Del"),
    ("del", "Del"),
    ("insert", "Ins"),
    ("ins", "Ins"),
    ("home", "Home"),
    ("end", "End"),
    ("pageup", "PgUp"),
    ("pgup", "PgUp"),
    ("pagedown", "PgDn"),
    ("pgdn", "PgDn"),
    ("up", "\u{2191}"),
    ("arrowup", "\u{2191}"),
    ("down", "\u{2193}"),
    ("arrowdown", "\u{2193}"),
    ("left", "\u{2190}"),
    ("arrowleft", "\u{2190}"),
    ("right", "\u{2192}"),
    ("arrowright", "\u{2192}"),
    ("plus", "+"),
    ("minus", "-"),
    ("dash", "-"),
    ("equals", "="),
    ("equal", "="),
    ("comma", ","),
    ("period", "."),
    ("dot", "."),
    ("slash", "/"),
    ("backslash", "\\"),
    ("semicolon", ";"),
    ("quote", "'"),
    ("backtick", "`"),
    ("grave", "`"),
    ("lbracket", "["),
    ("rbracket", "]"),
];

/// Split a chord on `+`, with one exception: a `+` where a key is expected
/// is the plus key itself, so `"ctrl++"` is Ctrl and +. Whitespace around a
/// token is dropped, but whitespace inside one is kept — a key may be
/// written "Page Up".
fn split_chord(shortcut: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut token = String::new();
    // Whether the current token is still empty, which is what makes the
    // difference between a separator and the plus key.
    let mut fresh = true;
    for ch in shortcut.chars() {
        if ch == '+' {
            if fresh {
                token.push('+');
                fresh = false;
            } else {
                out.push(token.trim().to_string());
                token.clear();
                fresh = true;
            }
        } else if ch.is_whitespace() {
            if !token.is_empty() {
                token.push(ch);
            }
        } else {
            token.push(ch);
            fresh = false;
        }
    }
    let last = token.trim();
    if !last.is_empty() {
        out.push(last.to_string());
    }
    out
}

/// The face of a key that is not a modifier: a name this module knows, else
/// the token exactly as it was written.
///
/// Passing an unknown key through rather than dropping it is the point. A
/// shortcut list is written by hand, and a widget that silently swallowed
/// what it did not recognise would leave a row with a cap missing and no way
/// to see why.
fn key_cap(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if let Some(face) = NAMED_KEYS
        .iter()
        .find(|(name, _)| *name == lower.as_str())
        .map(|(_, face)| *face)
    {
        return face.to_string();
    }
    // A single letter, and the function keys, are printed the way a keyboard
    // prints them whatever case they were typed in.
    if token.chars().count() == 1 {
        return token.to_uppercase();
    }
    if let Some(number) = lower.strip_prefix('f') {
        if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) {
            return token.to_uppercase();
        }
    }
    token.to_string()
}

/// Whether this build's platform prints its modifiers as symbols.
pub fn modifiers_are_symbols() -> bool {
    cfg!(target_vendor = "apple")
}

/// The caps a shortcut string turns into, for a platform that writes its
/// modifiers as `symbols` or as words.
///
/// The modifiers come first, in that platform's own order however they were
/// written; every other key follows in the order it was written. Taken
/// separately so a test can check both vocabularies from either machine.
pub fn shortcut_caps_for(shortcut: &str, symbols: bool) -> Vec<String> {
    let mut mods: Vec<Modifier> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for token in split_chord(shortcut) {
        match Modifier::from_name(&token.to_ascii_lowercase()) {
            // The same modifier written twice is one key on the keyboard and
            // one cap here.
            Some(modifier) => {
                if !mods.contains(&modifier) {
                    mods.push(modifier);
                }
            }
            None => keys.push(key_cap(&token)),
        }
    }
    let order = if symbols { SYMBOL_ORDER } else { WORD_ORDER };
    let mut caps: Vec<String> = order
        .iter()
        .filter(|modifier| mods.contains(modifier))
        .map(|modifier| modifier.cap(symbols).to_string())
        .collect();
    caps.extend(keys);
    caps
}

/// The caps a shortcut string turns into on the platform this build runs on.
pub fn shortcut_caps(shortcut: &str) -> Vec<String> {
    shortcut_caps_for(shortcut, modifiers_are_symbols())
}

/// How wide one cap is: the text plus its padding, never under the floor, so
/// a single letter is a key rather than a sliver.
pub fn cap_width(text_width: f64, pad_x: f64, min_width: f64) -> f64 {
    (text_width + pad_x * 2.0).max(min_width)
}

/// How wide a whole chord is: the caps, the separators, and a gap either
/// side of every separator. One place, because the drawing and anything that
/// places a chord — a right-aligned help row — must agree to the pixel.
pub fn chord_width(caps: &[f64], separator: f64, gap: f64) -> f64 {
    if caps.is_empty() {
        return 0.0;
    }
    caps.iter().sum::<f64>() + (caps.len() - 1) as f64 * (gap * 2.0 + separator)
}

/// One line of a help list, as written in `entries`.
#[derive(Clone, Debug, PartialEq)]
pub enum HelpEntry {
    /// A group title: everything after it belongs to it, until the next one.
    Heading(String),
    Row { label: String, shortcut: String },
}

/// Read the lines a host wrote into the list it draws.
///
/// A line starting `#` is a heading. Otherwise the FIRST `=` splits the
/// label from the chord, so a chord may contain one of its own:
/// `"Zoom in = ctrl+="` is a row, not a puzzle. A line with no `=` at all is
/// a row with no chord — something the app can do that has no shortcut yet,
/// which is worth listing and worth searching.
pub fn parse_help_entries(lines: &[String]) -> Vec<HelpEntry> {
    lines
        .iter()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            if let Some(heading) = line.strip_prefix('#') {
                return Some(HelpEntry::Heading(heading.trim().to_string()));
            }
            Some(match line.split_once('=') {
                Some((label, shortcut)) => HelpEntry::Row {
                    label: label.trim().to_string(),
                    shortcut: shortcut.trim().to_string(),
                },
                None => HelpEntry::Row {
                    label: line.to_string(),
                    shortcut: String::new(),
                },
            })
        })
        .collect()
}

/// Which entries a search leaves, by their index, in the order they were
/// written.
///
/// A row matches on its label or on the chord AS WRITTEN — "ctrl" finds the
/// row whether this build draws that cap as a word or as a symbol, which is
/// what someone typing into the box means by it.
///
/// A heading survives only with the first row that survives under it, so a
/// filtered list never shows a group title with nothing beneath it. A query
/// that matches the heading itself keeps the whole group: asking for
/// "editing" is asking for what is in it.
pub fn visible_entries(entries: &[HelpEntry], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..entries.len()).collect();
    }
    let mut out = Vec::new();
    let mut heading: Option<usize> = None;
    let mut heading_matched = false;
    for (index, entry) in entries.iter().enumerate() {
        match entry {
            HelpEntry::Heading(text) => {
                heading = Some(index);
                heading_matched = text.to_lowercase().contains(&query);
                if heading_matched {
                    out.push(index);
                }
            }
            HelpEntry::Row { label, shortcut } => {
                let hit = heading_matched
                    || label.to_lowercase().contains(&query)
                    || shortcut.to_lowercase().contains(&query);
                if hit {
                    if !heading_matched {
                        if let Some(at) = heading.take() {
                            out.push(at);
                        }
                    }
                    out.push(index);
                }
            }
        }
    }
    out
}

/// The cap's face and the state of the key under it. `pressed` and `shelf`
/// ride in the struct rather than the DSL because the widget places the
/// glyph against the same two numbers: what the shader draws and where the
/// letter lands cannot be allowed to disagree.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawKbdCap {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pressed: f32,
    #[live]
    shelf: f32,
}

/// A chord's own rect, painted as nothing.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawKbdGround {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook, Widget)]
pub struct Kbd {
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
    draw_bg: DrawKbdCap,
    #[live]
    draw_text: DrawText,

    /// What is printed on the cap.
    #[live]
    pub text: String,
    /// Drawn as if the key were held down.
    #[live]
    pub pressed: bool,
    /// The least a cap may be wide.
    #[live(22.0)]
    pub min_width: f64,
    /// Room either side of the text.
    #[live(7.0)]
    pub pad_x: f64,
    /// The height a cap takes when its walk fixes none.
    #[live(22.0)]
    pub cap_height: f64,
    /// The side of the key, seen from above.
    #[live]
    pub shelf: f64,
    #[live(true)]
    #[visible]
    visible: bool,
}

impl Kbd {
    /// The size a cap takes for `text` at `height`.
    pub fn cap_size(&self, cx: &mut Cx2d, text: &str, height: f64) -> DVec2 {
        let text_width = if text.is_empty() {
            0.0
        } else {
            measure(&self.draw_text, cx, text)
        };
        dvec2(cap_width(text_width, self.pad_x, self.min_width), height)
    }

    /// Draw one cap in `rect` with `text` on it, whatever this widget's own
    /// text is. A group draws every cap of a chord through here, so a lone
    /// cap and a cap in a chord are the same cap.
    pub fn draw_cap(&mut self, cx: &mut Cx2d, rect: Rect, text: &str) {
        self.hand_state_to_shader();
        self.draw_bg.draw_abs(cx, rect);
        self.draw_glyph(cx, rect, text);
    }

    fn hand_state_to_shader(&mut self) {
        self.draw_bg.pressed = if self.pressed { 1.0 } else { 0.0 };
        self.draw_bg.shelf = self.shelf as f32;
    }

    fn draw_glyph(&mut self, cx: &mut Cx2d, rect: Rect, text: &str) {
        if text.is_empty() {
            return;
        }
        let size = self.draw_text.text_style.font_size as f64;
        let width = measure(&self.draw_text, cx, text);
        // The glyph rides the face, so a held key takes its letter down with
        // it rather than leaving it hanging over the hole.
        let sink = if self.pressed { self.shelf } else { 0.0 };
        let pos = dvec2(
            rect.pos.x + (rect.size.x - width) * 0.5,
            ink_y(rect, size) + sink,
        );
        self.draw_text.draw_abs(cx, pos, text);
    }

    pub fn set_pressed(&mut self, cx: &mut Cx, pressed: bool) {
        if self.pressed != pressed {
            self.pressed = pressed;
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for Kbd {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let text = self.text.clone();
        let height = match walk.height {
            Size::Fixed(height) => height,
            _ => self.cap_height,
        };
        let size = self.cap_size(cx, &text, height);
        self.hand_state_to_shader();
        // draw_walk rather than draw_abs: a cap is normally one item in a
        // row, and it has to take its room in the turtle like any other.
        let rect = self.draw_bg.draw_walk(cx, sized(walk, size.x, size.y));
        self.draw_glyph(cx, rect, &text);
        DrawStep::done()
    }

    fn text(&self) -> String {
        self.text.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        if self.text != v {
            self.text = v.to_string();
            self.draw_bg.redraw(cx);
        }
    }
}

impl KbdRef {
    pub fn set_pressed(&self, cx: &mut Cx, pressed: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_pressed(cx, pressed);
        }
    }

    pub fn pressed(&self) -> bool {
        self.borrow().map(|inner| inner.pressed).unwrap_or(false)
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct KbdGroup {
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
    draw_bg: DrawKbdGround,
    #[live]
    draw_sep: DrawText,
    /// The one cap, drawn once per key in the chord.
    #[live]
    cap: Kbd,

    /// The chord, written with `+`: "ctrl+shift+P".
    #[live]
    pub shortcut: String,
    /// What goes between two caps; empty for nothing.
    #[live("+".to_string())]
    pub separator: String,
    /// Room either side of a separator.
    #[live(3.0)]
    pub gap: f64,
    #[live(22.0)]
    pub cap_height: f64,
    #[live(true)]
    #[visible]
    visible: bool,
}

impl KbdGroup {
    /// The caps this chord shows, in the order they are drawn, named as this
    /// platform names them.
    pub fn caps(&self) -> Vec<String> {
        shortcut_caps(&self.shortcut)
    }

    fn separator_width(&self, cx: &mut Cx2d) -> f64 {
        if self.separator.is_empty() {
            0.0
        } else {
            measure(&self.draw_sep, cx, &self.separator)
        }
    }

    /// The size the chord takes at `height`.
    pub fn chord_size(&self, cx: &mut Cx2d, height: f64) -> DVec2 {
        let caps = self.caps();
        let widths: Vec<f64> = caps
            .iter()
            .map(|text| self.cap.cap_size(cx, text, height).x)
            .collect();
        dvec2(
            chord_width(&widths, self.separator_width(cx), self.gap),
            height,
        )
    }

    /// Draw the caps and the separators inside `rect`, left to right. The
    /// walk is laid out by the same arithmetic `chord_size` reports, so a
    /// caller that placed the chord by that number gets what it placed.
    pub fn draw_chord(&mut self, cx: &mut Cx2d, rect: Rect) {
        let caps = self.caps();
        let separator = self.separator.clone();
        let separator_width = self.separator_width(cx);
        let separator_size = self.draw_sep.text_style.font_size as f64;
        let mut x = rect.pos.x;
        for (index, text) in caps.iter().enumerate() {
            if index > 0 {
                if !separator.is_empty() {
                    let pos = dvec2(x + self.gap, ink_y(rect, separator_size));
                    self.draw_sep.draw_abs(cx, pos, &separator);
                }
                x += self.gap * 2.0 + separator_width;
            }
            let width = self.cap.cap_size(cx, text, rect.size.y).x;
            let cap_rect = Rect {
                pos: dvec2(x, rect.pos.y),
                size: dvec2(width, rect.size.y),
            };
            self.cap.draw_cap(cx, cap_rect, text);
            x += width;
        }
    }

    pub fn set_shortcut(&mut self, cx: &mut Cx, shortcut: &str) {
        if self.shortcut != shortcut {
            self.shortcut = shortcut.to_string();
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for KbdGroup {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let height = match walk.height {
            Size::Fixed(height) => height,
            _ => self.cap_height,
        };
        let size = self.chord_size(cx, height);
        self.draw_bg.begin(cx, sized(walk, size.x, size.y), self.layout);
        let rect = cx.turtle().rect();
        self.draw_chord(cx, rect);
        self.draw_bg.end(cx);
        DrawStep::done()
    }

    /// The chord as it is drawn, so a test reads the caps rather than the
    /// string they came from.
    fn text(&self) -> String {
        self.caps().join(&self.separator)
    }

    /// Takes a shortcut string, the same one the DSL takes.
    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_shortcut(cx, v);
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.caps().join(&self.separator))
    }
}

impl KbdGroupRef {
    pub fn set_shortcut(&self, cx: &mut Cx, shortcut: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_shortcut(cx, shortcut);
        }
    }

    /// The caps as drawn on this platform.
    pub fn caps(&self) -> Vec<String> {
        self.borrow().map(|inner| inner.caps()).unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum ShortcutHelpAction {
    /// A row was pressed: its label and the chord as it was written.
    Picked { label: String, shortcut: String },
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct ShortcutHelp {
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
    /// The band under the pointer, drawn only for the row it is over.
    #[live]
    draw_row: DrawQuad,
    #[live]
    draw_heading: DrawText,
    #[live]
    draw_label: DrawText,
    /// One chord, drawn once per row.
    #[live]
    chord: KbdGroup,
    #[live]
    search: TextInput,

    /// The lines: "# Heading" opens a group, "Label = chord" is a row.
    #[live]
    pub entries: Vec<String>,
    /// Shown in place of the rows when the search matches nothing.
    #[live("Nothing matches".to_string())]
    pub empty_text: String,
    #[live(10.0)]
    pub pad: f64,
    #[live(24.0)]
    pub row_height: f64,
    #[live(28.0)]
    pub heading_height: f64,
    /// The height of the search field; 0 leaves it out, for a host that
    /// filters the list from its own box.
    #[live(26.0)]
    pub search_height: f64,
    #[live(8.0)]
    pub search_gap: f64,
    #[live(20.0)]
    pub cap_height: f64,
    #[live(true)]
    #[visible]
    visible: bool,

    #[rust]
    query: String,
    /// Where each drawn row landed, and which entry it is.
    #[rust]
    rows: Vec<(usize, Rect)>,
    #[rust]
    hover: Option<usize>,
}

impl ShortcutHelp {
    fn model(&self) -> Vec<HelpEntry> {
        parse_help_entries(&self.entries)
    }

    /// How many shortcut rows the search leaves; a test waits on this rather
    /// than on pixels.
    pub fn visible_rows(&self) -> usize {
        let entries = self.model();
        visible_entries(&entries, &self.query)
            .into_iter()
            .filter(|index| matches!(entries[*index], HelpEntry::Row { .. }))
            .count()
    }

    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<String>) {
        if self.entries != entries {
            self.entries = entries;
            self.hover = None;
            self.redraw(cx);
        }
    }

    /// Filter the list. The search field is set along with the model, so a
    /// query pushed by the host and one typed into the box leave the widget
    /// in the same state.
    pub fn set_query(&mut self, cx: &mut Cx, query: &str) {
        if self.query != query {
            self.query = query.to_string();
            // A filtered list has different rows under the pointer than the
            // one that was there when it was last hovered.
            self.hover = None;
            if self.search.text() != query {
                self.search.set_text(cx, query);
            }
            self.redraw(cx);
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }
}

impl Widget for ShortcutHelp {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let entries = self.model();
        let shown = visible_entries(&entries, &self.query);

        // Fit asks for exactly what will be drawn: the panel has no scroll
        // of its own, so a host that gives it a fixed height is choosing to
        // cut the list off, and one that gives it Fit gets all of it.
        let mut height = self.pad * 2.0;
        if self.search_height > 0.0 {
            height += self.search_height + self.search_gap;
        }
        if shown.is_empty() {
            height += self.row_height;
        } else {
            for index in &shown {
                height += match &entries[*index] {
                    HelpEntry::Heading(_) => self.heading_height,
                    HelpEntry::Row { .. } => self.row_height,
                };
            }
        }

        self.draw_bg
            .begin(cx, sized(walk, DEFAULT_HELP_WIDTH, height), self.layout);
        let panel = cx.turtle().rect();
        let left = panel.pos.x + self.pad;
        let width = (panel.size.x - self.pad * 2.0).max(0.0);
        let mut y = panel.pos.y + self.pad;

        if self.search_height > 0.0 {
            let walk = Walk {
                abs_pos: Some(dvec2(left, y)),
                width: Size::Fixed(width),
                height: Size::Fixed(self.search_height),
                ..Walk::default()
            };
            let _ = self.search.draw_walk(cx, scope, walk);
            y += self.search_height + self.search_gap;
        }

        self.rows.clear();
        if shown.is_empty() {
            let rect = Rect {
                pos: dvec2(left, y),
                size: dvec2(width, self.row_height),
            };
            let size = self.draw_heading.text_style.font_size as f64;
            let text = self.empty_text.clone();
            // The quiet voice, which is the heading's: this is not a row,
            // and drawing it in the row's ink would read as one.
            self.draw_heading
                .draw_abs(cx, dvec2(left, ink_y(rect, size)), &text);
        }

        for index in &shown {
            match &entries[*index] {
                HelpEntry::Heading(text) => {
                    let rect = Rect {
                        pos: dvec2(left, y),
                        size: dvec2(width, self.heading_height),
                    };
                    let size = self.draw_heading.text_style.font_size as f64;
                    self.draw_heading
                        .draw_abs(cx, dvec2(left, ink_y(rect, size)), text);
                    y += self.heading_height;
                }
                HelpEntry::Row { label, shortcut } => {
                    let rect = Rect {
                        pos: dvec2(left, y),
                        size: dvec2(width, self.row_height),
                    };
                    if self.hover == Some(*index) {
                        self.draw_row.draw_abs(cx, rect);
                    }
                    let size = self.draw_label.text_style.font_size as f64;
                    self.draw_label
                        .draw_abs(cx, dvec2(left, ink_y(rect, size)), label);
                    // The chord is measured before it is placed: a row reads
                    // right to left from the keys, so they end at the edge.
                    self.chord.shortcut = shortcut.clone();
                    let chord = self.chord.chord_size(cx, self.cap_height);
                    let chord_rect = Rect {
                        pos: dvec2(
                            rect.pos.x + rect.size.x - chord.x,
                            rect.pos.y + (rect.size.y - chord.y) * 0.5,
                        ),
                        size: chord,
                    };
                    self.chord.draw_chord(cx, chord_rect);
                    self.rows.push((*index, rect));
                    y += self.row_height;
                }
            }
        }

        self.draw_bg.end(cx);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        for action in cx.capture_actions(|cx| self.search.handle_event(cx, event, scope)) {
            if let TextInputAction::Changed(text) = action.as_widget_action().cast() {
                self.set_query(cx, &text);
            }
        }
        let uid = self.uid;
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(fe) | Hit::FingerHoverOver(fe) => {
                // The search field's own rect is not a row, so a pointer over
                // it lands in none of these and lights nothing.
                let at = self
                    .rows
                    .iter()
                    .find(|(_, rect)| rect.contains(fe.abs))
                    .map(|(index, _)| *index);
                if at != self.hover {
                    self.hover = at;
                    cx.set_cursor(if at.is_some() {
                        MouseCursor::Hand
                    } else {
                        MouseCursor::Default
                    });
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                let Some((index, _)) = self.rows.iter().find(|(_, rect)| rect.contains(fe.abs))
                else {
                    return;
                };
                if let Some(HelpEntry::Row { label, shortcut }) = self.model().get(*index) {
                    let action = ShortcutHelpAction::Picked {
                        label: label.clone(),
                        shortcut: shortcut.clone(),
                    };
                    cx.widget_action(uid, action);
                }
            }
            _ => {}
        }
    }

    /// What is in the search box.
    fn text(&self) -> String {
        self.query.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_query(cx, v);
    }

    /// How many rows the search leaves.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.visible_rows().to_string())
    }
}

impl ShortcutHelpRef {
    pub fn set_entries(&self, cx: &mut Cx, entries: Vec<String>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_entries(cx, entries);
        }
    }

    pub fn set_query(&self, cx: &mut Cx, query: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_query(cx, query);
        }
    }

    pub fn visible_rows(&self) -> usize {
        self.borrow().map(|inner| inner.visible_rows()).unwrap_or(0)
    }

    /// The row pressed this pass: its label and the chord as it was written.
    pub fn picked(&self, actions: &Actions) -> Option<(String, String)> {
        let action = actions.find_widget_action(self.widget_uid())?;
        match action.cast::<ShortcutHelpAction>() {
            ShortcutHelpAction::Picked { label, shortcut } => Some((label, shortcut)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|row| row.to_string()).collect()
    }

    /// The order the modifiers were written in is the writer's business; the
    /// order they are drawn in is the platform's.
    #[test]
    fn modifiers_print_in_one_order_whatever_order_they_were_written_in() {
        assert_eq!(shortcut_caps_for("ctrl+shift+p", false), ["Ctrl", "Shift", "P"]);
        assert_eq!(shortcut_caps_for("shift+ctrl+p", false), ["Ctrl", "Shift", "P"]);
        assert_eq!(shortcut_caps_for("SHIFT+Ctrl+P", false), ["Ctrl", "Shift", "P"]);
    }

    /// Where the modifiers are symbols they are also read in a different
    /// order, so the same string comes out as a different chord.
    #[test]
    fn the_symbol_platform_gets_symbols_and_its_own_order() {
        assert_eq!(
            shortcut_caps_for("shift+cmd+z", true),
            ["\u{21e7}", "\u{2318}", "Z"]
        );
        assert_eq!(shortcut_caps_for("shift+cmd+z", false), ["Meta", "Shift", "Z"]);
        assert_eq!(
            shortcut_caps_for("ctrl+alt+delete", true),
            ["\u{2303}", "\u{2325}", "Del"]
        );
    }

    /// A key nobody here has heard of is printed as it was written. Dropping
    /// it would leave a row with a cap missing and no way to see why.
    #[test]
    fn an_unknown_key_is_printed_as_it_was_written() {
        assert_eq!(shortcut_caps_for("ctrl+Frobnicate", false), ["Ctrl", "Frobnicate"]);
        assert_eq!(shortcut_caps_for("ctrl+web", false), ["Ctrl", "web"]);
        // A modifier this module does not know is not a modifier: it becomes
        // a cap of its own rather than vanishing.
        assert_eq!(shortcut_caps_for("hyper+k", false), ["hyper", "K"]);
    }

    #[test]
    fn an_empty_shortcut_is_no_caps_at_all() {
        assert!(shortcut_caps_for("", false).is_empty());
        assert!(shortcut_caps_for("   ", false).is_empty());
        assert!(shortcut_caps_for("", true).is_empty());
    }

    #[test]
    fn a_bare_key_needs_no_modifier() {
        assert_eq!(shortcut_caps_for("k", false), ["K"]);
        assert_eq!(shortcut_caps_for("Escape", false), ["Esc"]);
        assert_eq!(shortcut_caps_for("f5", true), ["F5"]);
        assert_eq!(shortcut_caps_for("up", false), ["\u{2191}"]);
    }

    /// The separator is also a key. "ctrl++" is the zoom chord, not a typo.
    #[test]
    fn the_plus_key_survives_being_the_separator_as_well() {
        assert_eq!(shortcut_caps_for("ctrl++", false), ["Ctrl", "+"]);
        assert_eq!(shortcut_caps_for("ctrl+plus", false), ["Ctrl", "+"]);
        assert_eq!(shortcut_caps_for("+", false), ["+"]);
    }

    #[test]
    fn one_modifier_written_twice_is_one_cap() {
        assert_eq!(shortcut_caps_for("ctrl+control+s", false), ["Ctrl", "S"]);
    }

    #[test]
    fn a_named_key_takes_the_shape_a_keyboard_prints() {
        assert_eq!(shortcut_caps_for("ctrl+pgdn", false), ["Ctrl", "PgDn"]);
        assert_eq!(shortcut_caps_for("shift+tab", false), ["Shift", "Tab"]);
        assert_eq!(shortcut_caps_for("alt+f4", false), ["Alt", "F4"]);
    }

    #[test]
    fn a_chord_is_as_wide_as_its_caps_its_separators_and_its_gaps() {
        assert_eq!(chord_width(&[], 8.0, 3.0), 0.0);
        assert_eq!(chord_width(&[20.0], 8.0, 3.0), 20.0);
        assert_eq!(chord_width(&[20.0, 30.0], 8.0, 3.0), 64.0);
        // No separator still leaves the gaps: the caps must not touch.
        assert_eq!(chord_width(&[20.0, 30.0], 0.0, 2.0), 54.0);
    }

    #[test]
    fn a_cap_is_never_narrower_than_its_floor() {
        assert_eq!(cap_width(6.0, 7.0, 22.0), 22.0);
        assert_eq!(cap_width(40.0, 7.0, 22.0), 54.0);
    }

    #[test]
    fn a_hash_opens_a_group_and_the_first_equals_splits_a_row() {
        let entries = parse_help_entries(&lines(&[
            "# Editing",
            "Copy = ctrl+c",
            "  ",
            "Zoom in = ctrl+=",
            "Not bound yet",
        ]));
        assert_eq!(
            entries,
            vec![
                HelpEntry::Heading("Editing".to_string()),
                HelpEntry::Row { label: "Copy".to_string(), shortcut: "ctrl+c".to_string() },
                HelpEntry::Row { label: "Zoom in".to_string(), shortcut: "ctrl+=".to_string() },
                HelpEntry::Row { label: "Not bound yet".to_string(), shortcut: String::new() },
            ]
        );
    }

    #[test]
    fn an_empty_search_shows_everything() {
        let entries = parse_help_entries(&lines(&["# Editing", "Copy = ctrl+c"]));
        assert_eq!(visible_entries(&entries, ""), [0, 1]);
        assert_eq!(visible_entries(&entries, "   "), [0, 1]);
    }

    /// A row that survives brings its heading with it, and a heading with
    /// nothing left under it does not survive alone.
    #[test]
    fn a_search_keeps_the_heading_of_every_row_it_keeps() {
        let entries = parse_help_entries(&lines(&[
            "# Editing",
            "Copy = ctrl+c",
            "Paste = ctrl+v",
            "# View",
            "Zoom in = ctrl+plus",
        ]));
        assert_eq!(visible_entries(&entries, "paste"), [0, 2]);
        assert_eq!(visible_entries(&entries, "zoom"), [3, 4]);
        assert!(visible_entries(&entries, "nothing here").is_empty());
    }

    /// The chord is searched as it was WRITTEN, so "ctrl" finds the row on a
    /// build that draws that cap as a symbol.
    #[test]
    fn a_search_reads_the_chord_as_well_as_the_label() {
        let entries = parse_help_entries(&lines(&["Copy = ctrl+c", "Undo = ctrl+z"]));
        assert_eq!(visible_entries(&entries, "ctrl+z"), [1]);
    }

    #[test]
    fn a_search_that_matches_a_heading_keeps_the_whole_group() {
        let entries = parse_help_entries(&lines(&[
            "# Editing",
            "Copy = ctrl+c",
            "Paste = ctrl+v",
            "# View",
            "Zoom in = ctrl+plus",
        ]));
        assert_eq!(visible_entries(&entries, "editing"), [0, 1, 2]);
    }
}
