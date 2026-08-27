//! The mpterm terminal widget: renders a `Session` and feeds it input.
//!
//! Rendering follows the cell-grid discipline (ghostty's renderer notes):
//! integer cell advance from the monospace font, background rects merged
//! into per-row runs, glyphs batched into one instance buffer, underline
//! styles drawn by a dedicated shader, block/bar/underline cursor with a
//! hollow variant when unfocused.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use makepad_widgets::text::geom::Point;
use makepad_widgets::text::rasterizer::RasterizedGlyph;
use makepad_widgets::*;

use crate::session::Session;
use crate::term::color::Rgb;
use crate::term::key_encode::{
    encode_key, Key, KeyAction, KeyEncodeOptions, KeyEvent as TermKeyEvent, KeyMods, KittyFlags,
};
use crate::term::modes::Mode;
use crate::term::mouse_encode::{
    encode_mouse, MouseButton as TermMouseButton, MouseEventKind, MouseFormat, MouseReport,
    MouseTracking,
};
use crate::term::page::CellContent;
use crate::term::screen::CursorStyle;
use crate::term::style::{StyleColor, StyleFlags};
use crate::term::terminal::TermEvent;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawTermBg::script_shader(vm)) {
        ..mod.draw.DrawQuad
        draw_call_group: @term_cell_bg
        color: #x1a1b26
        pixel: fn() {
            return vec4(self.color.rgb * self.color.a, self.color.a)
        }
    }

    set_type_default() do #(DrawTermUnderline::script_shader(vm)) {
        ..mod.draw.DrawQuad
        draw_call_group: @term_underline
        color: #xc0caf5
        kind: 1.0
        on: fn(a: float) {
            return vec4(self.color.rgb * self.color.a * a, self.color.a * a)
        }
        pixel: fn() {
            if self.kind < 1.5 {
                return self.on(1.0)
            }
            if self.kind < 2.5 {
                // Double: two bands with a gap.
                if self.pos.y < 0.33 || self.pos.y > 0.66 {
                    return self.on(1.0)
                }
                return self.on(0.0)
            }
            if self.kind < 3.5 {
                // Curly: sine band.
                let center = 0.5 + sin(self.pos.x * self.rect_size.x * 1.2) * 0.3
                if abs(self.pos.y - center) < 0.22 {
                    return self.on(1.0)
                }
                return self.on(0.0)
            }
            if self.kind < 4.5 {
                // Dotted.
                if modf(self.pos.x * self.rect_size.x, 3.0) < 1.5 {
                    return self.on(1.0)
                }
                return self.on(0.0)
            }
            // Dashed.
            if modf(self.pos.x * self.rect_size.x, 8.0) < 5.0 {
                return self.on(1.0)
            }
            return self.on(0.0)
        }
    }

    set_type_default() do #(DrawTermCursor::script_shader(vm)) {
        ..mod.draw.DrawQuad
        draw_call_group: @term_cursor
        color: #xc0caf5
        hollow: 0.0
        pixel: fn() {
            if self.hollow < 0.5 {
                return vec4(self.color.rgb * self.color.a, self.color.a)
            }
            let border = 1.0
            let bx = border / self.rect_size.x
            let by = border / self.rect_size.y
            if self.pos.x < bx || self.pos.x > 1.0 - bx || self.pos.y < by || self.pos.y > 1.0 - by {
                return vec4(self.color.rgb, 1.0)
            }
            return vec4(0.0, 0.0, 0.0, 0.0)
        }
    }

    mod.widgets.MpTermBase = #(MpTerm::register_widget(vm))

    mod.widgets.MpTerm = set_type_default() do mod.widgets.MpTermBase {
        width: Fill
        height: Fill
        font_size: 10.0
        pad_x: 6.0
        pad_y: 4.0
        draw_bg +: {
            color: uniform(#x1a1b26)
            pixel: fn() {
                return vec4(self.color.rgb * self.color.a, self.color.a)
            }
        }
        draw_text +: {
            draw_call_group: @term_text
            text_style: theme.font_code
        }
        draw_cell_bg +: {}
        draw_underline +: {}
        draw_cursor +: {}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawTermBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawTermUnderline {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    kind: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawTermCursor {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    hollow: f32,
}

#[derive(Clone, Debug, Default)]
pub enum MpTermAction {
    TitleChanged(String),
    PwdChanged(String),
    Bell,
    Exited,
    #[default]
    None,
}

#[derive(Clone, Copy)]
struct CachedGlyph {
    rasterized: RasterizedGlyph,
    font_size_in_lpxs: f32,
    x_offset_in_lpxs: f32,
}

/// The tokyo-night terminal palette (mpterm's default look; makepad-wm
/// re-themes at spawn time via --theme args later).
pub fn default_theme() -> ([Rgb; 16], Rgb, Rgb) {
    let base16 = [
        Rgb::new(0x1a, 0x1b, 0x26), // black = background
        Rgb::new(0xf7, 0x76, 0x8e),
        Rgb::new(0x9e, 0xce, 0x6a),
        Rgb::new(0xe0, 0xaf, 0x68),
        Rgb::new(0x7a, 0xa2, 0xf7),
        Rgb::new(0xad, 0x8e, 0xe6),
        Rgb::new(0x44, 0x9d, 0xab),
        Rgb::new(0xa9, 0xb1, 0xd6), // white = foreground
        Rgb::new(0x41, 0x48, 0x68), // bright black = muted
        Rgb::new(0xff, 0x7a, 0x93),
        Rgb::new(0xb9, 0xf2, 0x7c),
        Rgb::new(0xff, 0x9e, 0x64),
        Rgb::new(0x7d, 0xa6, 0xff),
        Rgb::new(0xbb, 0x9a, 0xf7),
        Rgb::new(0x0d, 0xb9, 0xd7),
        Rgb::new(0xc0, 0xca, 0xf5), // bright white = bright fg
    ];
    (
        base16,
        Rgb::new(0xa9, 0xb1, 0xd6),
        Rgb::new(0x1a, 0x1b, 0x26),
    )
}

const SELECTION_COLOR: Vec4f = Vec4f {
    x: 0x29 as f32 / 255.0,
    y: 0x2e as f32 / 255.0,
    z: 0x42 as f32 / 255.0,
    w: 1.0,
};

#[derive(Script, Widget)]
pub struct MpTerm {
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
    draw_text: DrawText,
    #[live]
    draw_cell_bg: DrawTermBg,
    #[live]
    draw_underline: DrawTermUnderline,
    #[live]
    draw_cursor: DrawTermCursor,
    #[live(10.0)]
    font_size: f64,
    #[live(6.0)]
    pad_x: f64,
    #[live(4.0)]
    pad_y: f64,

    #[rust]
    session: Option<Session>,
    #[rust]
    pub cwd: Option<PathBuf>,
    /// A one-shot job instead of the interactive shell (`--preview` runs
    /// the pager on a file); the session ends when it exits.
    #[rust]
    pub command: Option<String>,
    #[rust]
    area: Area,
    #[rust]
    rect: Rect,
    #[rust]
    cell_w: f64,
    #[rust]
    cell_h: f64,
    #[rust]
    cell_baseline: f64,
    #[rust]
    glyph_cache: HashMap<char, Option<CachedGlyph>>,
    #[rust]
    glyph_cache_key: (u32, u64),
    /// Lines scrolled back from the bottom (0 = live).
    #[rust]
    view_offset: usize,
    // Selection in absolute (eviction-stable) rows.
    #[rust]
    sel_anchor: Option<(u64, usize)>,
    #[rust]
    sel_cursor: Option<(u64, usize)>,
    #[rust]
    selecting: bool,
    #[rust]
    last_finger: Option<Vec2d>,
    #[rust]
    select_scroll_frame: NextFrame,
    #[rust]
    bell_frames: u8,
    #[rust]
    last_mouse_cell: Option<(u32, u32, u8)>,
    /// Background alpha (focused, unfocused): Omarchy's window opacity rule
    /// "0.985 0.96", handed down by makepad-wm via MPTERM_OPACITY. Standalone
    /// runs are opaque. The shared swapchain is BGRA and the compositor
    /// blends premultiplied, so the wallpaper shows through for free.
    #[rust((1.0, 1.0))]
    bg_opacity: (f32, f32),
}

impl ScriptHook for MpTerm {}

impl MpTerm {
    fn ensure_session(&mut self, _cx: &mut Cx) {
        if self.session.is_some() {
            return;
        }
        let cols = 80;
        let rows = 24;
        if let Ok(spec) = std::env::var("MPTERM_OPACITY") {
            let mut it = spec.split_whitespace().filter_map(|s| s.parse::<f32>().ok());
            if let Some(active) = it.next() {
                let inactive = it.next().unwrap_or(active);
                self.bg_opacity = (active.clamp(0.0, 1.0), inactive.clamp(0.0, 1.0));
            }
        }
        match Session::spawn(
            cols,
            rows,
            self.cwd.as_deref(),
            None,
            self.command.as_deref(),
        ) {
            Ok(mut session) => {
                // makepad-wm hands the splash theme's terminal palette down
                // via MPTERM_COLORS; standalone runs use the bundled default.
                let (mut base16, mut fg, mut bg) = default_theme();
                if let Ok(env) = std::env::var("MPTERM_COLORS") {
                    for pair in env.split(';') {
                        let Some((key, value)) = pair.split_once('=') else {
                            continue;
                        };
                        let Some(rgb) = parse_hex_rgb(value) else {
                            continue;
                        };
                        if let Some(idx) = key.strip_prefix("color") {
                            if let Ok(i) = idx.parse::<usize>() {
                                if i < 16 {
                                    base16[i] = rgb;
                                }
                            }
                        } else if key == "foreground" {
                            fg = rgb;
                        } else if key == "background" {
                            bg = rgb;
                        } else if key == "cursor" {
                            session.terminal.cursor_color = Some(rgb);
                        }
                    }
                }
                session.terminal.set_theme(&base16, fg, bg);
                self.session = Some(session);
            }
            Err(err) => {
                error!("mpterm: failed to spawn shell: {}", err);
            }
        }
    }

    fn refresh_metrics(&mut self, cx: &mut Cx2d) {
        self.draw_text.text_style.font_size = self.font_size as f32;
        let key = (
            (self.font_size * 64.0) as u32,
            cx.current_dpi_factor().to_bits(),
        );
        if key != self.glyph_cache_key {
            self.glyph_cache.clear();
            self.glyph_cache_key = key;
        }
        if let Some(run) = self.draw_text.prepare_single_line_run(cx, "M") {
            let g = &run.glyphs[0];
            self.cell_w = g.advance_in_lpxs as f64;
            let glyph_h = (run.ascender_in_lpxs - run.descender_in_lpxs) as f64;
            self.cell_h = glyph_h * self.draw_text.text_style.line_spacing as f64;
            self.cell_baseline =
                (self.cell_h - glyph_h) * 0.5 + run.ascender_in_lpxs as f64;
        }
        if self.cell_w <= 0.0 {
            self.cell_w = self.font_size * 0.6;
        }
        if self.cell_h <= 0.0 {
            self.cell_h = self.font_size * 1.35;
        }
    }

    fn grid_size(&self) -> (usize, usize) {
        let cols = ((self.rect.size.x - self.pad_x * 2.0) / self.cell_w)
            .floor()
            .max(2.0) as usize;
        let rows = ((self.rect.size.y - self.pad_y * 2.0) / self.cell_h)
            .floor()
            .max(2.0) as usize;
        (cols, rows)
    }

    fn cached_glyph(&mut self, cx: &mut Cx2d, ch: char) -> Option<CachedGlyph> {
        if let Some(hit) = self.glyph_cache.get(&ch) {
            return *hit;
        }
        let mut buf = [0u8; 4];
        let text: &str = ch.encode_utf8(&mut buf);
        let prepared = self.draw_text.prepare_single_line_run(cx, text);
        let cached = prepared.and_then(|run| {
            run.glyphs.first().map(|g| CachedGlyph {
                rasterized: g.rasterized,
                font_size_in_lpxs: g.font_size_in_lpxs,
                x_offset_in_lpxs: g.pen_x_in_lpxs + g.offset_x_in_lpxs,
            })
        });
        self.glyph_cache.insert(ch, cached);
        cached
    }

    fn rgb_to_vec4(rgb: Rgb, alpha: f32) -> Vec4f {
        vec4(
            rgb.r as f32 / 255.0,
            rgb.g as f32 / 255.0,
            rgb.b as f32 / 255.0,
            alpha,
        )
    }

    /// Resolve a cell's fg/bg honoring inverse (cell + global DECSCNM),
    /// bold-brightening for the 8 base palette colors, faint and invisible.
    fn resolve_colors(
        session: &Session,
        style: &crate::term::style::Style,
        global_inverse: bool,
    ) -> (Option<Vec4f>, Option<Vec4f>) {
        let term = &session.terminal;
        let mut fg_color = style.fg_color;
        // Classic bold-brightens: palette 0-7 + bold renders 8-15.
        if style.flags.has(StyleFlags::BOLD) {
            if let StyleColor::Palette(i) = fg_color {
                if i < 8 {
                    fg_color = StyleColor::Palette(i + 8);
                }
            }
        }
        let mut fg = fg_color.resolve(&term.palette, term.default_fg);
        let mut bg = style.bg_color.resolve_opt(&term.palette);

        let inverse = style.flags.has(StyleFlags::INVERSE) != global_inverse;
        if inverse {
            let old_fg = fg;
            fg = bg.unwrap_or(term.default_bg);
            bg = Some(old_fg);
        }

        if style.flags.has(StyleFlags::INVISIBLE) {
            return (None, bg.map(|b| Self::rgb_to_vec4(b, 1.0)));
        }
        let alpha = if style.flags.has(StyleFlags::FAINT) {
            0.55
        } else {
            1.0
        };
        (
            Some(Self::rgb_to_vec4(fg, alpha)),
            bg.map(|b| Self::rgb_to_vec4(b, 1.0)),
        )
    }

    // --------------------------------------------------------------
    // Selection
    // --------------------------------------------------------------

    fn sel_ordered(&self) -> Option<((u64, usize), (u64, usize))> {
        let a = self.sel_anchor?;
        let c = self.sel_cursor?;
        if a == c {
            return None;
        }
        Some(if a <= c { (a, c) } else { (c, a) })
    }

    fn cell_selected(&self, abs_row: u64, col: usize) -> bool {
        let Some(((sr, sc), (er, ec))) = self.sel_ordered() else {
            return false;
        };
        if abs_row < sr || abs_row > er {
            return false;
        }
        if sr == er {
            return col >= sc && col < ec;
        }
        if abs_row == sr {
            return col >= sc;
        }
        if abs_row == er {
            return col < ec;
        }
        true
    }

    fn selected_text(&self) -> Option<String> {
        let session = self.session.as_ref()?;
        let screen = session.terminal.screen();
        let ((sr, sc), (er, ec)) = self.sel_ordered()?;
        let mut out = String::new();
        for abs in sr..=er {
            let Some(virt) = screen.virtual_of_absolute(abs) else {
                continue;
            };
            let Some(row) = screen.row_virtual(virt) else {
                continue;
            };
            let from = if abs == sr { sc } else { 0 };
            let to = if abs == er { ec } else { screen.cols };
            let mut line = String::new();
            for col in from..to.min(screen.cols) {
                if let Some(cell) = row.cell(col) {
                    cell.content.push_text(&mut line);
                } else {
                    line.push(' ');
                }
            }
            let line = line.trim_end();
            out.push_str(line);
            if abs < er {
                // A soft-wrapped row continues logically: no newline.
                if !row.wrapped {
                    out.push('\n');
                }
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// Abs row + col at a window position.
    fn pick(&self, abs_pos: Vec2d) -> Option<(u64, usize)> {
        let session = self.session.as_ref()?;
        let screen = session.terminal.screen();
        let local_x = abs_pos.x - self.rect.pos.x - self.pad_x;
        let local_y = abs_pos.y - self.rect.pos.y - self.pad_y;
        let col = (local_x / self.cell_w).floor().max(0.0) as usize;
        let col = col.min(screen.cols.saturating_sub(1));
        let visual_row = (local_y / self.cell_h).floor().max(0.0) as usize;
        let top_virtual = self.top_virtual_row();
        let virt = (top_virtual + visual_row).min(screen.total_rows().saturating_sub(1));
        Some((screen.absolute_of_virtual(virt), col))
    }

    /// First visible virtual row for the current view offset.
    fn top_virtual_row(&self) -> usize {
        let Some(session) = self.session.as_ref() else {
            return 0;
        };
        let screen = session.terminal.screen();
        screen.scrollback.len().saturating_sub(self.view_offset)
    }

    // --------------------------------------------------------------
    // Input
    // --------------------------------------------------------------

    fn key_opts(&self) -> KeyEncodeOptions {
        let Some(session) = self.session.as_ref() else {
            return KeyEncodeOptions::default();
        };
        let term = &session.terminal;
        KeyEncodeOptions {
            cursor_key_application: term.modes.get(Mode::CursorKeys),
            keypad_key_application: term.modes.get(Mode::KeypadKeys),
            ignore_keypad_with_numlock: term.modes.get(Mode::IgnoreKeypadWithNumlock),
            alt_esc_prefix: term.modes.get(Mode::AltEscPrefix),
            modify_other_keys_state_2: term.modify_other_keys == 2,
            kitty_flags: KittyFlags(term.kitty_flags()),
            backarrow_key_mode: term.modes.get(Mode::BackarrowKeyMode),
        }
    }

    fn mods_of(m: &KeyModifiers) -> KeyMods {
        KeyMods {
            shift: m.shift,
            ctrl: m.control,
            alt: m.alt,
            super_: m.logo,
            caps_lock: false,
            num_lock: false,
        }
    }

    fn send_key(
        &mut self,
        cx: &mut Cx,
        key: Key,
        mods: &KeyModifiers,
        action: KeyAction,
        utf8: &str,
        unshifted: u32,
    ) {
        let opts = self.key_opts();
        let event = TermKeyEvent {
            action,
            key,
            mods: Self::mods_of(mods),
            consumed_mods: KeyMods::default(),
            utf8: utf8.to_string(),
            unshifted_codepoint: unshifted,
        };
        let bytes = encode_key(&event, &opts);
        if !bytes.is_empty() {
            self.scroll_to_bottom();
            if let Some(session) = self.session.as_mut() {
                session.write(&bytes);
            }
            self.redraw(cx);
        }
    }

    fn paste(&mut self, cx: &mut Cx, text: &str) {
        let Some(bracketed) = self
            .session
            .as_ref()
            .map(|s| s.terminal.modes.get(Mode::BracketedPaste))
        else {
            return;
        };
        let mut bytes = Vec::with_capacity(text.len() + 16);
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
            // Sanitize: no escape bytes inside a bracketed paste body.
            bytes.extend(text.bytes().filter(|b| *b != 0x1b));
            bytes.extend_from_slice(b"\x1b[201~");
        } else {
            // Legacy paste: newlines become CR like every terminal.
            for b in text.bytes() {
                bytes.push(if b == b'\n' { b'\r' } else { b });
            }
        }
        self.scroll_to_bottom();
        if let Some(session) = self.session.as_mut() {
            session.write(&bytes);
        }
        self.redraw(cx);
    }

    fn scroll_to_bottom(&mut self) {
        self.view_offset = 0;
    }

    fn mouse_tracking(&self) -> (MouseTracking, MouseFormat) {
        let Some(session) = self.session.as_ref() else {
            return (MouseTracking::None, MouseFormat::X10);
        };
        let m = &session.terminal.modes;
        let tracking = if m.get(Mode::MouseEventAny) {
            MouseTracking::Any
        } else if m.get(Mode::MouseEventButton) {
            MouseTracking::Button
        } else if m.get(Mode::MouseEventNormal) {
            MouseTracking::Normal
        } else if m.get(Mode::MouseEventX10) {
            MouseTracking::X10
        } else {
            return (MouseTracking::None, MouseFormat::X10);
        };
        let format = if m.get(Mode::MouseFormatSgrPixels) {
            MouseFormat::SgrPixels
        } else if m.get(Mode::MouseFormatSgr) {
            MouseFormat::Sgr
        } else if m.get(Mode::MouseFormatUrxvt) {
            MouseFormat::Urxvt
        } else if m.get(Mode::MouseFormatUtf8) {
            MouseFormat::Utf8
        } else {
            MouseFormat::X10
        };
        (tracking, format)
    }

    fn mouse_cell(&self, abs: Vec2d) -> (u32, u32, u32, u32) {
        let x = (abs.x - self.rect.pos.x - self.pad_x).max(0.0);
        let y = (abs.y - self.rect.pos.y - self.pad_y).max(0.0);
        let col = (x / self.cell_w).floor() as u32;
        let row = (y / self.cell_h).floor() as u32;
        (col, row, x as u32, y as u32)
    }

    /// Send a mouse report if the app asked for it; true when consumed.
    fn report_mouse(
        &mut self,
        cx: &mut Cx,
        abs: Vec2d,
        kind: MouseEventKind,
        button: TermMouseButton,
        mods: &KeyModifiers,
    ) -> bool {
        let (tracking, format) = self.mouse_tracking();
        if tracking == MouseTracking::None || mods.shift {
            return false;
        }
        let (col, row, x_px, y_px) = self.mouse_cell(abs);
        // Motion dedup: only report when the cell (or button) changed.
        if kind == MouseEventKind::Motion {
            let sig = (col, row, button as u8);
            if self.last_mouse_cell == Some(sig) {
                return true;
            }
            self.last_mouse_cell = Some(sig);
        } else {
            self.last_mouse_cell = None;
        }
        let report = MouseReport {
            kind,
            button,
            mods: Self::mods_of(mods),
            col,
            row,
            x_px,
            y_px,
        };
        if let Some(bytes) = encode_mouse(&report, tracking, format) {
            if let Some(session) = self.session.as_mut() {
                session.write(&bytes);
                self.redraw(cx);
            }
        }
        true
    }

    fn handle_scroll(&mut self, cx: &mut Cx, e: &FingerScrollEvent) {
        let Some((alt_scroll, max)) = self.session.as_ref().map(|session| {
            let term = &session.terminal;
            (
                matches!(term.active, crate::term::terminal::ActiveScreen::Alternate)
                    && term.modes.get(Mode::MouseAlternateScroll),
                term.screen().scrollback.len(),
            )
        }) else {
            return;
        };
        let lines = if e.device.is_mouse() {
            (e.scroll.y / 40.0).abs().ceil().max(1.0) as usize
        } else {
            ((e.scroll.y.abs() / self.cell_h).ceil()).max(1.0) as usize
        };
        let down = e.scroll.y > 0.0;

        let (tracking, _) = self.mouse_tracking();
        if tracking != MouseTracking::None && !e.modifiers.shift {
            let button = if down {
                TermMouseButton::WheelDown
            } else {
                TermMouseButton::WheelUp
            };
            for _ in 0..lines.min(8) {
                self.report_mouse(cx, e.abs, MouseEventKind::Press, button, &e.modifiers);
            }
            return;
        }

        if alt_scroll {
            // Alternate scroll: wheel becomes arrow keys.
            let key = if down { Key::ArrowDown } else { Key::ArrowUp };
            for _ in 0..lines.min(8) {
                self.send_key(cx, key, &KeyModifiers::default(), KeyAction::Press, "", 0);
            }
            return;
        }

        // Scrollback.
        if down {
            self.view_offset = self.view_offset.saturating_sub(lines);
        } else {
            self.view_offset = (self.view_offset + lines).min(max);
        }
        self.redraw(cx);
    }

    fn map_keycode(kc: KeyCode) -> Option<Key> {
        Some(match kc {
            KeyCode::ReturnKey => Key::Enter,
            KeyCode::NumpadEnter => Key::NumpadEnter,
            KeyCode::Tab => Key::Tab,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Escape => Key::Escape,
            KeyCode::Delete => Key::Delete,
            KeyCode::Insert => Key::Insert,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            KeyCode::ArrowUp => Key::ArrowUp,
            KeyCode::ArrowDown => Key::ArrowDown,
            KeyCode::ArrowLeft => Key::ArrowLeft,
            KeyCode::ArrowRight => Key::ArrowRight,
            KeyCode::F1 => Key::F1,
            KeyCode::F2 => Key::F2,
            KeyCode::F3 => Key::F3,
            KeyCode::F4 => Key::F4,
            KeyCode::F5 => Key::F5,
            KeyCode::F6 => Key::F6,
            KeyCode::F7 => Key::F7,
            KeyCode::F8 => Key::F8,
            KeyCode::F9 => Key::F9,
            KeyCode::F10 => Key::F10,
            KeyCode::F11 => Key::F11,
            KeyCode::F12 => Key::F12,
            _ => return None,
        })
    }

    fn is_special(kc: KeyCode) -> bool {
        Self::map_keycode(kc).is_some()
    }

    // --------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------

    fn draw_terminal(&mut self, cx: &mut Cx2d) {
        // The session moves out of self for the draw so cached-glyph and
        // selection helpers can borrow self freely.
        let Some(mut session) = self.session.take() else {
            return;
        };
        self.draw_terminal_inner(cx, &mut session);
        self.session = Some(session);
    }

    fn draw_terminal_inner(&mut self, cx: &mut Cx2d, session: &mut Session) {
        session.terminal.dirty = false;

        let has_focus = cx.has_key_focus(self.area);
        let origin_x = self.rect.pos.x + self.pad_x;
        let origin_y = self.rect.pos.y + self.pad_y;
        let (cell_w, cell_h) = (self.cell_w, self.cell_h);
        let global_inverse = session.terminal.modes.get(Mode::ReverseColors);
        let cursor_visible = session.terminal.modes.get(Mode::CursorVisible);
        let (default_bg, default_fg, cursor_color, cursor_style) = {
            let t = &session.terminal;
            (
                t.default_bg,
                t.default_fg,
                t.cursor_color.unwrap_or(t.default_fg),
                t.cursor_style,
            )
        };

        // Background fill honoring DECSCNM.
        let bg_fill = if global_inverse {
            default_fg
        } else {
            default_bg
        };
        let alpha = if has_focus {
            self.bg_opacity.0
        } else {
            self.bg_opacity.1
        };
        let v = Self::rgb_to_vec4(bg_fill, alpha);
        self.draw_bg
            .draw_vars
            .set_uniform(cx, id!(color), &[v.x, v.y, v.z, v.w]);
        self.draw_bg.draw_abs(cx, self.rect);

        let screen = session.terminal.screen();
        let rows = screen.rows;
        let cols = screen.cols;
        let top_virtual = screen.scrollback.len().saturating_sub(self.view_offset);
        let total = screen.total_rows();

        // Collect draw data first (bg runs, glyphs, decorations), then issue
        // batched draws per layer.
        struct BgRun {
            x: f64,
            y: f64,
            w: f64,
            color: Vec4f,
        }
        struct GlyphDraw {
            x: f64,
            y: f64,
            ch: char,
            color: Vec4f,
            bold: bool,
        }
        struct DecoDraw {
            x: f64,
            y: f64,
            w: f64,
            color: Vec4f,
            kind: f32,
            strike: bool,
        }
        let mut bg_runs: Vec<BgRun> = Vec::new();
        let mut glyphs: Vec<GlyphDraw> = Vec::with_capacity(rows * cols / 2);
        let mut decos: Vec<DecoDraw> = Vec::new();

        for vis_row in 0..rows {
            let virt = top_virtual + vis_row;
            if virt >= total {
                break;
            }
            let abs = screen.absolute_of_virtual(virt);
            let row = match screen.row_virtual(virt) {
                Some(r) => r,
                None => continue,
            };
            let y = origin_y + vis_row as f64 * cell_h;
            let mut run: Option<(usize, usize, Vec4f)> = None;
            for col in 0..cols {
                let cell = row.cell(col);
                let (fg, bg) = match cell {
                    Some(c) => Self::resolve_colors(session, &c.style, global_inverse),
                    None => (None, None),
                };
                let selected = self.cell_selected(abs, col);
                let bg = if selected {
                    Some(SELECTION_COLOR)
                } else {
                    bg
                };

                // Merge bg runs.
                match (&mut run, bg) {
                    (Some((_, end, color)), Some(c)) if *color == c && *end == col => {
                        *end = col + 1;
                    }
                    (prev, next) => {
                        if let Some((start, end, color)) = prev.take() {
                            bg_runs.push(BgRun {
                                x: origin_x + start as f64 * cell_w,
                                y,
                                w: (end - start) as f64 * cell_w,
                                color,
                            });
                        }
                        if let Some(c) = next {
                            *prev = Some((col, col + 1, c));
                        }
                    }
                }

                let Some(cell) = cell else { continue };
                let Some(fg) = fg else { continue };

                // Decorations.
                let underline = cell.style.flags.underline();
                let strike = cell.style.flags.has(StyleFlags::STRIKETHROUGH);
                if underline != crate::term::style::Underline::None || strike {
                    let ul_color = cell
                        .style
                        .underline_color
                        .resolve_opt(&session.terminal.palette)
                        .map(|c| Self::rgb_to_vec4(c, 1.0))
                        .unwrap_or(fg);
                    let width = cell.content.width().max(1) as f64 * cell_w;
                    if underline != crate::term::style::Underline::None {
                        decos.push(DecoDraw {
                            x: origin_x + col as f64 * cell_w,
                            y,
                            w: width,
                            color: ul_color,
                            kind: underline as u8 as f32,
                            strike: false,
                        });
                    }
                    if strike {
                        decos.push(DecoDraw {
                            x: origin_x + col as f64 * cell_w,
                            y,
                            w: width,
                            color: fg,
                            kind: 1.0,
                            strike: true,
                        });
                    }
                }

                // Text.
                match &cell.content {
                    CellContent::Char(c) | CellContent::WideChar(c) => {
                        if *c != ' ' {
                            glyphs.push(GlyphDraw {
                                x: origin_x + col as f64 * cell_w,
                                y,
                                ch: *c,
                                color: fg,
                                bold: cell.style.flags.has(StyleFlags::BOLD),
                            });
                        }
                    }
                    CellContent::Cluster(cluster) => {
                        // First codepoint via the cache; combining marks are
                        // drawn over it.
                        for c in &cluster.cps {
                            glyphs.push(GlyphDraw {
                                x: origin_x + col as f64 * cell_w,
                                y,
                                ch: *c,
                                color: fg,
                                bold: cell.style.flags.has(StyleFlags::BOLD),
                            });
                        }
                    }
                    _ => {}
                }
            }
            if let Some((start, end, color)) = run.take() {
                bg_runs.push(BgRun {
                    x: origin_x + start as f64 * cell_w,
                    y,
                    w: (end - start) as f64 * cell_w,
                    color,
                });
            }
        }

        // Cursor (only when the live bottom is in view).
        let cursor = if self.view_offset == 0 && cursor_visible && !session.exited {
            let s = session.terminal.screen();
            Some((s.cursor.x.min(cols - 1), s.cursor.y))
        } else {
            None
        };

        // Layer 1: backgrounds.
        self.draw_cell_bg.new_draw_call(cx);
        for r in &bg_runs {
            self.draw_cell_bg.color = r.color;
            self.draw_cell_bg.draw_abs(
                cx,
                Rect {
                    pos: dvec2(r.x, r.y),
                    size: dvec2(r.w, cell_h),
                },
            );
        }

        // Layer 2: cursor under text (block) — text stays readable on top.
        if let Some((cx_col, cx_row)) = cursor {
            let x = origin_x + cx_col as f64 * cell_w;
            let y = origin_y + cx_row as f64 * cell_h;
            let color = Self::rgb_to_vec4(cursor_color, 1.0);
            self.draw_cursor.new_draw_call(cx);
            self.draw_cursor.color = color;
            self.draw_cursor.hollow = if has_focus { 0.0 } else { 1.0 };
            let (rect, hollow_override) = match cursor_style {
                CursorStyle::BlinkingBar | CursorStyle::SteadyBar => (
                    Rect {
                        pos: dvec2(x, y),
                        size: dvec2((cell_w * 0.15).max(1.5), cell_h),
                    },
                    Some(0.0),
                ),
                CursorStyle::BlinkingUnderline | CursorStyle::SteadyUnderline => (
                    Rect {
                        pos: dvec2(x, y + cell_h - (cell_h * 0.12).max(2.0)),
                        size: dvec2(cell_w, (cell_h * 0.12).max(2.0)),
                    },
                    Some(0.0),
                ),
                _ => (
                    Rect {
                        pos: dvec2(x, y),
                        size: dvec2(cell_w, cell_h),
                    },
                    None,
                ),
            };
            if let Some(h) = hollow_override {
                if !has_focus {
                    // Non-block cursors just dim when unfocused.
                    self.draw_cursor.color = vec4(color.x, color.y, color.z, 0.5);
                }
                self.draw_cursor.hollow = h;
            }
            self.draw_cursor.draw_abs(cx, rect);
        }

        // Layer 3: glyphs, one batch.
        self.draw_text.new_draw_call(cx);
        self.draw_text.begin_many_instances(cx);
        let baseline = self.cell_baseline;
        for g in &glyphs {
            // A block cursor inverts the glyph on top of it for contrast.
            let mut color = g.color;
            if let Some((ccol, crow)) = cursor {
                let gx = ((g.x - origin_x) / cell_w).round() as usize;
                let gy = ((g.y - origin_y) / cell_h).round() as usize;
                if gx == ccol
                    && gy == crow
                    && has_focus
                    && matches!(
                        cursor_style,
                        CursorStyle::Default
                            | CursorStyle::BlinkingBlock
                            | CursorStyle::SteadyBlock
                    )
                {
                    color = Self::rgb_to_vec4(default_bg, 1.0);
                }
            }
            if let Some(glyph) = self.cached_glyph(cx, g.ch) {
                let point = Point::new(
                    (g.x + glyph.x_offset_in_lpxs as f64) as f32,
                    (g.y + baseline) as f32,
                );
                self.draw_text.draw_rasterized_glyph_abs(
                    cx,
                    point,
                    glyph.font_size_in_lpxs,
                    glyph.rasterized,
                    color,
                );
                if g.bold {
                    // Synthetic bold: second strike, half-pixel offset.
                    self.draw_text.draw_rasterized_glyph_abs(
                        cx,
                        Point::new(point.x + 0.5, point.y),
                        glyph.font_size_in_lpxs,
                        glyph.rasterized,
                        color,
                    );
                }
            }
        }
        self.draw_text.end_many_instances(cx);

        // Layer 4: decorations.
        if !decos.is_empty() {
            self.draw_underline.new_draw_call(cx);
            for d in &decos {
                self.draw_underline.color = d.color;
                self.draw_underline.kind = d.kind;
                let (dy, dh) = if d.strike {
                    (d.y + cell_h * 0.5 - 1.0, 1.5)
                } else if d.kind >= 2.5 && d.kind < 3.5 {
                    // Curly gets a taller band.
                    (d.y + cell_h - 4.0, 4.0)
                } else if d.kind >= 1.5 && d.kind < 2.5 {
                    (d.y + cell_h - 4.0, 4.0)
                } else {
                    (d.y + cell_h - 2.0, 1.5)
                };
                self.draw_underline.draw_abs(
                    cx,
                    Rect {
                        pos: dvec2(d.x, dy),
                        size: dvec2(d.w, dh),
                    },
                );
            }
        }

        // Bell flash.
        if self.bell_frames > 0 {
            self.bell_frames -= 1;
            self.draw_cell_bg.new_draw_call(cx);
            self.draw_cell_bg.color =
                vec4(1.0, 1.0, 1.0, 0.06 * self.bell_frames as f32);
            self.draw_cell_bg.draw_abs(cx, self.rect);
            self.draw_bg.redraw(cx);
        }

        // Scrollback position indicator.
        if self.view_offset > 0 {
            let sb = session.terminal.screen().scrollback.len().max(1);
            let frac = self.view_offset as f64 / sb as f64;
            let h = (self.rect.size.y * 0.2).max(24.0);
            let track = self.rect.size.y - h;
            let y = self.rect.pos.y + track * (1.0 - frac);
            self.draw_cell_bg.new_draw_call(cx);
            self.draw_cell_bg.color = vec4(1.0, 1.0, 1.0, 0.25);
            self.draw_cell_bg.draw_abs(
                cx,
                Rect {
                    pos: dvec2(self.rect.pos.x + self.rect.size.x - 4.0, y),
                    size: dvec2(3.0, h),
                },
            );
        }

        // Exited banner.
        if session.exited {
            self.draw_cell_bg.new_draw_call(cx);
            self.draw_cell_bg.color = vec4(0.0, 0.0, 0.0, 0.5);
            self.draw_cell_bg.draw_abs(cx, self.rect);
        }
    }

    fn pump_session(&mut self, cx: &mut Cx) {
        let mut actions: Vec<MpTermAction> = Vec::new();
        let mut needs_redraw = false;
        if let Some(session) = self.session.as_mut() {
            if session.drain() {
                needs_redraw = true;
            }
            // Inside mpwm, shell facts go to the compositor through the WM
            // API so it can title the tile and open new terminals in our
            // cwd (the Omarchy behavior).
            for event in session.take_events() {
                match event {
                    TermEvent::TitleChanged(title) => {
                        // Hosted in mpwm: the bar shows it (no-op standalone).
                        mp_wm_api::set_title(cx, &title);
                        actions.push(MpTermAction::TitleChanged(title))
                    }
                    TermEvent::Bell => {
                        self.bell_frames = 6;
                        actions.push(MpTermAction::Bell);
                        needs_redraw = true;
                    }
                    TermEvent::ClipboardSet { text, .. } => {
                        cx.copy_to_clipboard(&text);
                    }
                    TermEvent::PwdChanged(url) => {
                        // file://host/path -> path
                        let path = url
                            .strip_prefix("file://")
                            .map(|rest| match rest.find('/') {
                                Some(idx) => rest[idx..].to_string(),
                                None => rest.to_string(),
                            })
                            .unwrap_or(url);
                        self.cwd = Some(PathBuf::from(&path));
                        // Hosted: new terminals open here (Omarchy's
                        // terminal-in-cwd); no-op standalone.
                        mp_wm_api::set_cwd(cx, Path::new(&path));
                        actions.push(MpTermAction::PwdChanged(path));
                    }
                    TermEvent::Notification { .. } => {}
                }
            }
            if session.exited {
                actions.push(MpTermAction::Exited);
            }
        }
        for action in actions {
            cx.widget_action(self.uid, action);
        }
        if needs_redraw {
            self.draw_bg.redraw(cx);
        }
    }
}

impl Widget for MpTerm {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        self.rect = cx.turtle().rect();
        self.refresh_metrics(cx);
        self.ensure_session(cx);

        let (cols, rows) = self.grid_size();
        if let Some(session) = self.session.as_mut() {
            session.resize(cols, rows);
            session.drain();
        }

        self.draw_terminal(cx);

        cx.end_turtle_with_area(&mut self.area);
        if self.session.is_some() && cx.has_key_focus(self.area) {
            let s = self
                .session
                .as_ref()
                .map(|s| {
                    let sc = s.terminal.screen();
                    (sc.cursor.x, sc.cursor.y)
                })
                .unwrap_or((0, 0));
            let ime = dvec2(
                self.pad_x + s.0 as f64 * self.cell_w,
                self.pad_y + (s.1 + 1) as f64 * self.cell_h,
            );
            cx.show_text_ime(self.area, ime);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::Signal = event {
            // No check_and_clear here: the event loop already consumed the
            // global flag to dispatch this event, so a second check would
            // only steal a signal raised since — including the one our own
            // budget-capped drain re-arms to continue a flood next tick.
            self.pump_session(cx);
        }

        // Edge auto-scroll while selecting.
        if self.selecting && self.select_scroll_frame.is_event(event).is_some() {
            self.select_scroll_frame = cx.new_next_frame();
            if let Some(abs) = self.last_finger {
                let band = self.cell_h.max(6.0);
                let top = self.rect.pos.y + band;
                let bottom = self.rect.pos.y + self.rect.size.y - band;
                let max = self
                    .session
                    .as_ref()
                    .map(|s| s.terminal.screen().scrollback.len())
                    .unwrap_or(0);
                if abs.y < top {
                    self.view_offset = (self.view_offset + 1).min(max);
                } else if abs.y > bottom {
                    self.view_offset = self.view_offset.saturating_sub(1);
                }
                if let Some(pos) = self.pick(abs) {
                    self.sel_cursor = Some(pos);
                }
                self.draw_bg.redraw(cx);
            }
        }

        match event.hits(cx, self.area) {
            Hit::FingerDown(e) => {
                cx.set_key_focus(self.area);
                if self.report_mouse(
                    cx,
                    e.abs,
                    MouseEventKind::Press,
                    TermMouseButton::Left,
                    &e.modifiers,
                ) {
                    return;
                }
                if let Some(pos) = self.pick(e.abs) {
                    if e.tap_count >= 3 {
                        // Line selection.
                        self.sel_anchor = Some((pos.0, 0));
                        self.sel_cursor = Some((
                            pos.0,
                            self.session
                                .as_ref()
                                .map(|s| s.terminal.cols())
                                .unwrap_or(80),
                        ));
                        self.selecting = false;
                    } else if e.tap_count == 2 {
                        let (start, end) = self.word_range(pos);
                        self.sel_anchor = Some((pos.0, start));
                        self.sel_cursor = Some((pos.0, end));
                        self.selecting = false;
                    } else {
                        self.sel_anchor = Some(pos);
                        self.sel_cursor = Some(pos);
                        self.selecting = true;
                        self.last_finger = Some(e.abs);
                        self.select_scroll_frame = cx.new_next_frame();
                    }
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerMove(e) => {
                if self.report_mouse(
                    cx,
                    e.abs,
                    MouseEventKind::Motion,
                    TermMouseButton::Left,
                    &e.modifiers,
                ) {
                    return;
                }
                if self.selecting {
                    if let Some(pos) = self.pick(e.abs) {
                        self.sel_cursor = Some(pos);
                    }
                    self.last_finger = Some(e.abs);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(e) => {
                self.report_mouse(
                    cx,
                    e.abs,
                    MouseEventKind::Release,
                    TermMouseButton::Left,
                    &e.modifiers,
                );
                self.selecting = false;
                self.last_finger = None;
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Text);
            }
            Hit::FingerScroll(e) => {
                self.handle_scroll(cx, &e);
            }
            Hit::KeyFocus(_) => {
                if let Some(session) = self.session.as_mut() {
                    if session.terminal.modes.get(Mode::FocusEvent) {
                        session.write(b"\x1b[I");
                    }
                }
                self.draw_bg.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                if let Some(session) = self.session.as_mut() {
                    if session.terminal.modes.get(Mode::FocusEvent) {
                        session.write(b"\x1b[O");
                    }
                }
                cx.hide_text_ime();
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(e) => {
                // Clear selection on typing.
                if Self::is_special(e.key_code) {
                    self.sel_anchor = None;
                    self.sel_cursor = None;
                    let key = Self::map_keycode(e.key_code).unwrap();
                    let action = if e.is_repeat {
                        KeyAction::Repeat
                    } else {
                        KeyAction::Press
                    };
                    self.send_key(cx, key, &e.modifiers, action, "", 0);
                } else if e.modifiers.control && !e.modifiers.logo {
                    if let Some(ch) = e.key_code.to_char(e.modifiers.shift) {
                        let key = letter_key(ch).unwrap_or(Key::Unidentified);
                        let action = if e.is_repeat {
                            KeyAction::Repeat
                        } else {
                            KeyAction::Press
                        };
                        self.send_key(
                            cx,
                            key,
                            &e.modifiers,
                            action,
                            "",
                            ch.to_ascii_lowercase() as u32,
                        );
                    }
                }
            }
            Hit::TextInput(e) => {
                if e.replace_last {
                    return;
                }
                if e.was_paste {
                    self.paste(cx, &e.input);
                } else {
                    let filtered: String = e
                        .input
                        .chars()
                        .filter(|c| *c != '\n' && *c != '\r')
                        .collect();
                    if !filtered.is_empty() {
                        self.sel_anchor = None;
                        self.sel_cursor = None;
                        self.scroll_to_bottom();
                        if let Some(session) = self.session.as_mut() {
                            session.write(filtered.as_bytes());
                        }
                        self.redraw(cx);
                    }
                }
            }
            Hit::TextCopy(e) => {
                if let Some(text) = self.selected_text() {
                    *e.response.borrow_mut() = Some(text);
                }
            }
            _ => {}
        }
    }
}

impl MpTerm {
    fn word_range(&self, pos: (u64, usize)) -> (usize, usize) {
        let Some(session) = self.session.as_ref() else {
            return (pos.1, pos.1 + 1);
        };
        let screen = session.terminal.screen();
        let Some(virt) = screen.virtual_of_absolute(pos.0) else {
            return (pos.1, pos.1 + 1);
        };
        let Some(row) = screen.row_virtual(virt) else {
            return (pos.1, pos.1 + 1);
        };
        let kind_of = |col: usize| -> Option<bool> {
            let c = row.cell(col).and_then(|c| c.content.primary())?;
            if c.is_whitespace() {
                None
            } else {
                Some(c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/')
            }
        };
        let Some(kind) = kind_of(pos.1) else {
            return (pos.1, pos.1 + 1);
        };
        let mut start = pos.1;
        while start > 0 && kind_of(start - 1) == Some(kind) {
            start -= 1;
        }
        let mut end = pos.1 + 1;
        while end < screen.cols && kind_of(end) == Some(kind) {
            end += 1;
        }
        (start, end)
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }
}

fn parse_hex_rgb(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    Some(Rgb::new(
        u8::from_str_radix(&s[0..2], 16).ok()?,
        u8::from_str_radix(&s[2..4], 16).ok()?,
        u8::from_str_radix(&s[4..6], 16).ok()?,
    ))
}

fn letter_key(ch: char) -> Option<Key> {
    Some(match ch.to_ascii_lowercase() {
        'a' => Key::KeyA,
        'b' => Key::KeyB,
        'c' => Key::KeyC,
        'd' => Key::KeyD,
        'e' => Key::KeyE,
        'f' => Key::KeyF,
        'g' => Key::KeyG,
        'h' => Key::KeyH,
        'i' => Key::KeyI,
        'j' => Key::KeyJ,
        'k' => Key::KeyK,
        'l' => Key::KeyL,
        'm' => Key::KeyM,
        'n' => Key::KeyN,
        'o' => Key::KeyO,
        'p' => Key::KeyP,
        'q' => Key::KeyQ,
        'r' => Key::KeyR,
        's' => Key::KeyS,
        't' => Key::KeyT,
        'u' => Key::KeyU,
        'v' => Key::KeyV,
        'w' => Key::KeyW,
        'x' => Key::KeyX,
        'y' => Key::KeyY,
        'z' => Key::KeyZ,
        ' ' => Key::Space,
        '[' => Key::BracketLeft,
        ']' => Key::BracketRight,
        '\\' => Key::Backslash,
        '/' => Key::Slash,
        '-' => Key::Minus,
        '=' => Key::Equal,
        ';' => Key::Semicolon,
        '\'' => Key::Quote,
        ',' => Key::Comma,
        '.' => Key::Period,
        '`' => Key::Backquote,
        '0'..='9' => match ch {
            '0' => Key::Digit0,
            '1' => Key::Digit1,
            '2' => Key::Digit2,
            '3' => Key::Digit3,
            '4' => Key::Digit4,
            '5' => Key::Digit5,
            '6' => Key::Digit6,
            '7' => Key::Digit7,
            '8' => Key::Digit8,
            _ => Key::Digit9,
        },
        _ => return None,
    })
}
