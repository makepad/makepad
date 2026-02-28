use crate::makepad_widgets::text::geom::Point;
use crate::makepad_widgets::text::rasterizer::RasterizedGlyph;
use crate::{app::AppData, makepad_widgets::*};
use makepad_terminal_core::{Color, CursorShape, StyleFlags, TermKeyCode, Terminal};
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawTerminalCellBg::script_shader(vm)) {
        ..mod.draw.DrawQuad
        draw_call_group: @cell_bg
        color: #x3a3d41
        pixel: fn() {
            return vec4(self.color.rgb * self.color.a, self.color.a)
        }
    }

    set_type_default() do #(DrawTerminalDecor::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: #xc5c8c6
        pixel: fn() {
            return vec4(self.color.rgb * self.color.a, self.color.a)
        }
    }

    set_type_default() do #(DrawTerminalCursor::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: #fff7
        color_unfocused: #fff7
        focus: 0.0
        border_width: 1.0
        pixel: fn() {
            if self.focus > 0.5 {
                return vec4(self.color.rgb * self.color.a, self.color.a)
            }
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let inset = self.border_width * 0.5
            let color = self.color_unfocused
            sdf.box(
                inset
                inset
                self.rect_size.x - self.border_width
                self.rect_size.y - self.border_width
                0.5
            )
            sdf.stroke(color, self.border_width)
            return sdf.result
        }
    }

    mod.widgets.DesktopTerminalViewBase = #(DesktopTerminalView::register_widget(vm))

    mod.widgets.DesktopTerminalView = set_type_default() do mod.widgets.DesktopTerminalViewBase {
        width: Fill
        height: Fill
        font_size: 9.0
        cell_width_factor: 0.6
        cell_height_factor: 1.4
        pad_x: 4.0
        pad_y: 2.0
        text_y_offset: 0.0
        cursor_y_offset: 0.0
        bold_is_bright: true
        faint_factor: 0.75
        selection_color_focus: theme.color_outset_active
        selection_color_unfocus: theme.color_outset_active * 0.65
        draw_bg +: {
            color: uniform(#x1d1f21)
            pixel: fn() {
                return self.color
            }
        }
        draw_text +: {
            draw_call_group: @text
            text_style: theme.font_code
        }
        draw_cell_bg +: {}
        draw_decor +: {}
        draw_cursor +: {}
    }
}

#[derive(Clone, Debug, Default)]
pub enum DesktopTerminalViewAction {
    Input {
        path: String,
        data: Vec<u8>,
    },
    Resize {
        path: String,
        cols: u16,
        rows: u16,
    },
    #[default]
    None,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawTerminalCellBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawTerminalDecor {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawTerminalCursor {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    color_unfocused: Vec4f,
    #[live]
    focus: f32,
    #[live]
    border_width: f32,
}

#[derive(Clone, Copy)]
struct CachedTerminalGlyph {
    rasterized: RasterizedGlyph,
    font_size_in_lpxs: f32,
    x_offset_in_lpxs: f32,
    baseline_offset_in_lpxs: f32,
}

#[derive(Script, Widget)]
pub struct DesktopTerminalView {
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
    draw_cursor: DrawTerminalCursor,
    #[live]
    draw_cell_bg: DrawTerminalCellBg,
    #[live]
    draw_decor: DrawTerminalDecor,
    #[live(9.0)]
    font_size: f64,
    #[live(0.6)]
    cell_width_factor: f64,
    #[live(1.4)]
    cell_height_factor: f64,
    #[live(4.0)]
    pad_x: f64,
    #[live(2.0)]
    pad_y: f64,
    #[live(0.0)]
    text_y_offset: f64,
    #[live(0.0)]
    cursor_y_offset: f64,
    #[live(true)]
    bold_is_bright: bool,
    #[live(0.75)]
    faint_factor: f64,
    #[live]
    selection_color_focus: Vec4f,
    #[live]
    selection_color_unfocus: Vec4f,
    #[rust]
    terminal: Option<Terminal>,
    #[rust]
    area: Area,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    last_size: (usize, usize),
    #[rust]
    current_path: Option<String>,
    #[rust]
    consumed_stream_len: usize,
    #[rust]
    cell_width: f64,
    #[rust]
    cell_height: f64,
    #[rust]
    cell_offset_y: f64,
    #[rust]
    glyph_cache: HashMap<char, CachedTerminalGlyph>,
    #[rust]
    glyph_cache_font_size: f32,
    #[rust]
    glyph_cache_font_scale: f32,
    #[rust]
    glyph_cache_dpi_factor: f64,
    #[rust]
    cursor_blink_timer: Timer,
    #[rust]
    cursor_blink_on: bool,
    #[rust]
    selection_anchor: Option<(usize, usize)>,
    #[rust]
    selection_cursor: Option<(usize, usize)>,
    #[rust]
    selecting: bool,
}

impl ScriptHook for DesktopTerminalView {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.cursor_blink_timer = cx.start_interval(0.45);
        });
    }
}

impl DesktopTerminalView {
    fn terminal_path_for_widget(cx: &Cx, data: &AppData, widget_uid: WidgetUid) -> Option<String> {
        let mount = data.active_mount.as_ref()?;
        let tabs = data.mount_terminal_tab_to_path.get(mount)?;
        let path = cx.widget_tree().path_to(widget_uid);
        for node in path.iter().rev() {
            if let Some(terminal_path) = tabs.get(node) {
                return Some(terminal_path.clone());
            }
        }
        None
    }

    fn ensure_terminal(&mut self) {
        if self.terminal.is_none() {
            self.terminal = Some(Terminal::new(120, 32));
            self.last_size = (120, 32);
        }
    }

    fn reset_terminal_for_path(&mut self, path: &str) {
        let (cols, rows) = self.last_size;
        self.terminal = Some(Terminal::new(cols.max(1), rows.max(1)));
        self.current_path = Some(path.to_string());
        self.consumed_stream_len = 0;
        self.cursor_blink_on = true;
        self.selection_anchor = None;
        self.selection_cursor = None;
        self.selecting = false;
    }

    fn fallback_cell_metrics(&self) -> (f64, f64) {
        let w = (self.font_size * self.cell_width_factor).max(1.0);
        let h = (self.font_size * self.cell_height_factor).max(1.0);
        (w, h)
    }

    fn refresh_cell_metrics(&mut self, cx: &mut Cx2d) {
        self.draw_text.text_style.font_size = self.font_size as f32;
        let (fallback_w, fallback_h) = self.fallback_cell_metrics();

        let layout = self
            .draw_text
            .layout(cx, 0.0, 0.0, None, false, Align::default(), "M");
        let Some(first_row) = layout.rows.first() else {
            self.cell_width = fallback_w;
            self.cell_height = fallback_h;
            self.cell_offset_y = 0.0;
            return;
        };
        let Some(first_glyph) = first_row.glyphs.first() else {
            self.cell_width = fallback_w;
            self.cell_height = fallback_h;
            self.cell_offset_y = 0.0;
            return;
        };

        let width_in_lpxs = first_glyph.advance_in_lpxs();
        let glyph_h_in_lpxs = first_glyph.ascender_in_lpxs() - first_glyph.descender_in_lpxs();
        let line_spacing_in_lpxs = glyph_h_in_lpxs * self.draw_text.text_style.line_spacing;

        self.cell_width = if width_in_lpxs > 0.0 {
            width_in_lpxs as f64
        } else {
            fallback_w
        };
        self.cell_height = if line_spacing_in_lpxs > 0.0 {
            line_spacing_in_lpxs as f64
        } else {
            fallback_h
        };
        self.cell_offset_y = ((self.cell_height - glyph_h_in_lpxs as f64) * 0.5).max(0.0);
    }

    fn cell_metrics(&self) -> (f64, f64) {
        let (fallback_w, fallback_h) = self.fallback_cell_metrics();
        (
            if self.cell_width > 0.0 {
                self.cell_width
            } else {
                fallback_w
            },
            if self.cell_height > 0.0 {
                self.cell_height
            } else {
                fallback_h
            },
        )
    }

    fn invalidate_glyph_cache_if_needed(&mut self, cx: &Cx2d) {
        let font_size = self.draw_text.text_style.font_size;
        let font_scale = self.draw_text.font_scale;
        let dpi_factor = cx.current_dpi_factor();
        if self.glyph_cache_font_size.to_bits() == font_size.to_bits()
            && self.glyph_cache_font_scale.to_bits() == font_scale.to_bits()
            && self.glyph_cache_dpi_factor.to_bits() == dpi_factor.to_bits()
        {
            return;
        }
        self.glyph_cache.clear();
        self.glyph_cache_font_size = font_size;
        self.glyph_cache_font_scale = font_scale;
        self.glyph_cache_dpi_factor = dpi_factor;
    }

    fn cached_terminal_glyph(&mut self, cx: &mut Cx2d, ch: char) -> Option<CachedTerminalGlyph> {
        if let Some(cached) = self.glyph_cache.get(&ch) {
            return Some(*cached);
        }
        let mut utf8 = [0u8; 4];
        let text = ch.encode_utf8(&mut utf8);
        let run = self.draw_text.prepare_single_line_run(cx, text)?;
        let glyph = run.glyphs.first()?;
        let cached = CachedTerminalGlyph {
            rasterized: glyph.rasterized,
            font_size_in_lpxs: glyph.font_size_in_lpxs,
            x_offset_in_lpxs: glyph.pen_x_in_lpxs + glyph.offset_x_in_lpxs,
            baseline_offset_in_lpxs: run.ascender_in_lpxs,
        };
        self.glyph_cache.insert(ch, cached);
        Some(cached)
    }

    fn sync_stream_from_data(&mut self, cx: &mut Cx, data: &mut AppData, path: &str) {
        self.ensure_terminal();
        if self.current_path.as_deref() != Some(path) {
            self.reset_terminal_for_path(path);
        }

        let Some(stream) = data.terminal_stream_by_path.get(path) else {
            return;
        };
        if self.consumed_stream_len > stream.len() {
            self.reset_terminal_for_path(path);
        }

        let chunk = &stream[self.consumed_stream_len..];
        if chunk.is_empty() {
            return;
        }
        if let Some(terminal) = &mut self.terminal {
            terminal.process_bytes(chunk);
            let outbound = terminal.take_outbound();
            if !outbound.is_empty() {
                cx.widget_action(
                    self.widget_uid(),
                    DesktopTerminalViewAction::Input {
                        path: path.to_string(),
                        data: outbound,
                    },
                );
            }
        }
        self.consumed_stream_len = stream.len();
    }

    fn update_terminal_size(&mut self, cx: &mut Cx, path: Option<&str>) {
        let (cell_width, cell_height) = self.cell_metrics();
        let rect = self.viewport_rect;
        let cols = ((rect.size.x - self.pad_x * 2.0) / cell_width)
            .floor()
            .max(1.0) as usize;
        let rows = ((rect.size.y - self.pad_y * 2.0) / cell_height)
            .floor()
            .max(1.0) as usize;
        if (cols, rows) == self.last_size {
            return;
        }
        self.last_size = (cols, rows);
        if let Some(terminal) = &mut self.terminal {
            terminal.resize(cols.max(1), rows.max(1));
        }
        if let Some(path) = path {
            cx.widget_action(
                self.widget_uid(),
                DesktopTerminalViewAction::Resize {
                    path: path.to_string(),
                    cols: cols as u16,
                    rows: rows as u16,
                },
            );
        }
    }

    fn draw_terminal(&mut self, cx: &mut Cx2d) {
        let Some(terminal) = self.terminal.as_ref() else {
            return;
        };
        let cols = terminal.screen().cols();
        if cols == 0 {
            return;
        }

        let (cell_width, cell_height) = self.cell_metrics();
        let rows_visible = ((self.viewport_rect.size.y - self.pad_y * 2.0) / cell_height)
            .floor()
            .max(1.0) as usize;
        let total_rows = terminal.screen().total_rows();
        let top_row = total_rows.saturating_sub(rows_visible);

        let origin_x = self.viewport_rect.pos.x + self.pad_x;
        let origin_y = self.viewport_rect.pos.y + self.pad_y;

        let palette = terminal.palette.colors;
        let default_fg = terminal.default_fg;
        let default_bg = terminal.default_bg;
        let cursor_visible = terminal.modes.cursor_visible;
        let cursor = terminal.cursor().clone();
        let scrollback_len = terminal.screen().scrollback_len();
        let blank_cell = makepad_terminal_core::Cell::default();
        let has_focus = cx.has_key_focus(self.area);

        let _ = terminal;
        self.draw_cell_bg.new_draw_call(cx);
        self.draw_cursor.new_draw_call(cx);
        self.draw_text.new_draw_call(cx);
        self.draw_decor.new_draw_call(cx);
        self.draw_text.begin_many_instances(cx);
        self.invalidate_glyph_cache_if_needed(cx);

        for virtual_row in top_row..total_rows {
            let row_cells = self.terminal.as_ref().and_then(|t| {
                t.screen()
                    .row_slice_virtual(virtual_row)
                    .map(|row| row.to_vec())
            });
            let y = origin_y + (virtual_row - top_row) as f64 * cell_height;
            for col in 0..cols {
                let cell = row_cells
                    .as_ref()
                    .and_then(|row| row.get(col))
                    .unwrap_or(&blank_cell);
                let mut fg_src = cell.style.fg;
                let mut bg_src = cell.style.bg;
                let flags = cell.style.flags;
                if flags.has(StyleFlags::INVERSE) {
                    std::mem::swap(&mut fg_src, &mut bg_src);
                }
                let mut fg_color = fg_src.resolve(&palette, default_fg);
                let bg_color = bg_src.resolve(&palette, default_bg);
                if self.bold_is_bright && flags.has(StyleFlags::BOLD) {
                    if let Color::Palette(idx) = fg_src {
                        if idx < 8 {
                            fg_color = palette[(idx + 8) as usize];
                        }
                    }
                }
                let mut fg_r = fg_color.r;
                let mut fg_g = fg_color.g;
                let mut fg_b = fg_color.b;
                if flags.has(StyleFlags::FAINT) {
                    fg_r = ((fg_r as f64 * self.faint_factor).round()).clamp(0.0, 255.0) as u8;
                    fg_g = ((fg_g as f64 * self.faint_factor).round()).clamp(0.0, 255.0) as u8;
                    fg_b = ((fg_b as f64 * self.faint_factor).round()).clamp(0.0, 255.0) as u8;
                }
                if flags.has(StyleFlags::INVISIBLE) {
                    fg_r = bg_color.r;
                    fg_g = bg_color.g;
                    fg_b = bg_color.b;
                }

                let x = origin_x + col as f64 * cell_width;
                let selected = self.is_cell_selected(virtual_row, col);
                if selected {
                    self.draw_cell_bg.color = if has_focus {
                        self.selection_color_focus
                    } else {
                        self.selection_color_unfocus
                    };
                    self.draw_cell_bg.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, y),
                            size: dvec2(cell_width, cell_height),
                        },
                    );
                } else if bg_color != default_bg {
                    self.draw_cell_bg.color = vec4(
                        bg_color.r as f32 / 255.0,
                        bg_color.g as f32 / 255.0,
                        bg_color.b as f32 / 255.0,
                        1.0,
                    );
                    self.draw_cell_bg.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, y),
                            size: dvec2(cell_width, cell_height),
                        },
                    );
                }

                let ch = cell.codepoint;
                let blink_hidden = flags.has(StyleFlags::BLINK) && !self.cursor_blink_on;
                if ch != ' ' && ch != '\0' && !blink_hidden {
                    let color = vec4(
                        fg_r as f32 / 255.0,
                        fg_g as f32 / 255.0,
                        fg_b as f32 / 255.0,
                        1.0,
                    );
                    if let Some(glyph) = self.cached_terminal_glyph(cx, ch) {
                        let baseline_y = y
                            + self.cell_offset_y
                            + self.text_y_offset
                            + glyph.baseline_offset_in_lpxs as f64;
                        self.draw_text.draw_rasterized_glyph_abs(
                            cx,
                            Point::new(
                                (x + glyph.x_offset_in_lpxs as f64) as f32,
                                baseline_y as f32,
                            ),
                            glyph.font_size_in_lpxs,
                            glyph.rasterized,
                            color,
                        );
                    } else {
                        let mut s = [0u8; 4];
                        let text = ch.encode_utf8(&mut s);
                        self.draw_text.color = color;
                        self.draw_text.draw_abs(
                            cx,
                            dvec2(x, y + self.cell_offset_y + self.text_y_offset),
                            text,
                        );
                    }
                }
            }
        }
        self.draw_text.end_many_instances(cx);

        if cursor_visible {
            let cursor_virtual_y = scrollback_len + cursor.y;
            if cursor_virtual_y >= top_row {
                let cursor_screen_row = cursor_virtual_y - top_row;
                let cx_x = origin_x + cursor.x as f64 * cell_width;
                let cx_y = origin_y + cursor_screen_row as f64 * cell_height + self.cursor_y_offset;
                self.draw_cursor.focus = if has_focus { 1.0 } else { 0.0 };
                let cursor_rect = if has_focus {
                    match cursor.shape {
                        CursorShape::Block => Rect {
                            pos: dvec2(cx_x, cx_y),
                            size: dvec2(cell_width, cell_height),
                        },
                        CursorShape::Bar => Rect {
                            pos: dvec2(cx_x, cx_y),
                            size: dvec2(2.0, cell_height),
                        },
                        CursorShape::Underline => Rect {
                            pos: dvec2(cx_x, cx_y + cell_height - 2.0),
                            size: dvec2(cell_width, 2.0),
                        },
                    }
                } else {
                    Rect {
                        pos: dvec2(cx_x, cx_y),
                        size: dvec2(cell_width, cell_height),
                    }
                };
                self.draw_cursor.draw_abs(cx, cursor_rect);
            }
        }
    }

    fn emit_input_bytes(&self, cx: &mut Cx, path: &str, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        cx.widget_action(
            self.widget_uid(),
            DesktopTerminalViewAction::Input {
                path: path.to_string(),
                data,
            },
        );
    }

    fn send_key_to_terminal(
        &mut self,
        cx: &mut Cx,
        path: &str,
        key_code: KeyCode,
        mods: &KeyModifiers,
    ) {
        let Some(terminal) = &self.terminal else {
            return;
        };
        let tc_key = map_keycode(key_code);
        if tc_key == TermKeyCode::None {
            return;
        }
        if let Some(bytes) = terminal.encode_key(tc_key, "", mods.shift, mods.control, mods.alt) {
            self.emit_input_bytes(cx, path, bytes);
        }
    }

    fn send_text_to_terminal(&mut self, cx: &mut Cx, path: &str, text: &str, mods: &KeyModifiers) {
        let Some(terminal) = &self.terminal else {
            return;
        };
        if let Some(bytes) =
            terminal.encode_key(TermKeyCode::None, text, mods.shift, mods.control, mods.alt)
        {
            self.emit_input_bytes(cx, path, bytes);
        }
    }

    fn emit_paste_text(&mut self, cx: &mut Cx, path: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        let bracketed = self
            .terminal
            .as_ref()
            .map(|t| t.modes.bracketed_paste)
            .unwrap_or(false);
        let mut bytes = Vec::with_capacity(text.len() + 16);
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.emit_input_bytes(cx, path, bytes);
    }

    fn shell_quote_path(path: &str) -> String {
        let mut out = String::with_capacity(path.len() + 2);
        out.push('\'');
        for ch in path.chars() {
            if ch == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(ch);
            }
        }
        out.push('\'');
        out
    }

    fn hex_nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(10 + (byte - b'a')),
            b'A'..=b'F' => Some(10 + (byte - b'A')),
            _ => None,
        }
    }

    fn decode_percent_escapes(input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let (Some(hi), Some(lo)) = (
                    Self::hex_nibble(bytes[i + 1]),
                    Self::hex_nibble(bytes[i + 2]),
                ) {
                    out.push((hi << 4) | lo);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8(out).unwrap_or_else(|_| input.to_string())
    }

    fn dropped_text_payload(items: &[DragItem]) -> Option<String> {
        if items.is_empty() {
            return None;
        }
        let mut payload_parts = Vec::new();
        let mut only_paths = true;
        for item in items {
            match item {
                DragItem::String { value, .. } => {
                    only_paths = false;
                    payload_parts.push(value.clone());
                }
                DragItem::FilePath { path, .. } => {
                    let decoded = Self::decode_percent_escapes(path);
                    payload_parts.push(Self::shell_quote_path(&decoded));
                }
            }
        }
        if payload_parts.is_empty() {
            None
        } else if only_paths {
            Some(format!("{} ", payload_parts.join(" ")))
        } else if payload_parts.len() == 1 {
            payload_parts.into_iter().next()
        } else {
            Some(payload_parts.join("\n"))
        }
    }

    fn is_clipboard_paste_shortcut(key_code: KeyCode, modifiers: &KeyModifiers) -> bool {
        matches!(key_code, KeyCode::KeyV) && (modifiers.control || modifiers.logo) && !modifiers.alt
    }

    fn is_special_pty_key(key_code: KeyCode) -> bool {
        matches!(
            key_code,
            KeyCode::ReturnKey
                | KeyCode::NumpadEnter
                | KeyCode::Backspace
                | KeyCode::Tab
                | KeyCode::Escape
                | KeyCode::Delete
                | KeyCode::ArrowUp
                | KeyCode::ArrowDown
                | KeyCode::ArrowLeft
                | KeyCode::ArrowRight
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Insert
                | KeyCode::F1
                | KeyCode::F2
                | KeyCode::F3
                | KeyCode::F4
                | KeyCode::F5
                | KeyCode::F6
                | KeyCode::F7
                | KeyCode::F8
                | KeyCode::F9
                | KeyCode::F10
                | KeyCode::F11
                | KeyCode::F12
        )
    }

    fn visible_rows_and_top_row(&self) -> Option<(usize, usize)> {
        let terminal = self.terminal.as_ref()?;
        let (_, cell_height) = self.cell_metrics();
        let rows_visible = ((self.viewport_rect.size.y - self.pad_y * 2.0) / cell_height)
            .floor()
            .max(1.0) as usize;
        let top_row = terminal.screen().total_rows().saturating_sub(rows_visible);
        Some((rows_visible, top_row))
    }

    fn pick(&self, abs: Vec2d) -> (usize, usize) {
        let Some(terminal) = self.terminal.as_ref() else {
            return (0, 0);
        };
        let cols = terminal.screen().cols().max(1);
        let total_rows = terminal.screen().total_rows().max(1);
        let (cell_width, cell_height) = self.cell_metrics();
        let origin_x = self.viewport_rect.pos.x + self.pad_x;
        let origin_y = self.viewport_rect.pos.y + self.pad_y;
        let (_, top_row) = self.visible_rows_and_top_row().unwrap_or((1, 0));

        let col = ((abs.x - origin_x) / cell_width).floor().max(0.0) as usize;
        let row = ((abs.y - origin_y) / cell_height).floor().max(0.0) as usize;
        let col = col.min(cols.saturating_sub(1));
        let row = (top_row + row).min(total_rows.saturating_sub(1));
        (row, col)
    }

    fn normalized_selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.selection_anchor?;
        let cursor = self.selection_cursor?;
        if anchor <= cursor {
            Some((anchor, cursor))
        } else {
            Some((cursor, anchor))
        }
    }

    fn is_cell_selected(&self, row: usize, col: usize) -> bool {
        let Some(((start_row, start_col), (end_row, end_col))) = self.normalized_selection_range()
        else {
            return false;
        };
        if row < start_row || row > end_row {
            return false;
        }
        if start_row == end_row {
            return col >= start_col && col <= end_col;
        }
        if row == start_row {
            return col >= start_col;
        }
        if row == end_row {
            return col <= end_col;
        }
        true
    }

    fn selected_text(&self) -> Option<String> {
        let terminal = self.terminal.as_ref()?;
        let cols = terminal.screen().cols();
        if cols == 0 {
            return None;
        }
        let Some(((start_row, start_col), (end_row, end_col))) = self.normalized_selection_range()
        else {
            return None;
        };

        let mut out = String::new();
        for row in start_row..=end_row {
            let row_cells = terminal.screen().row_slice_virtual(row)?;
            if row_cells.is_empty() {
                if row < end_row {
                    out.push('\n');
                }
                continue;
            }
            let from_col = if row == start_row { start_col } else { 0 };
            let to_col = if row == end_row {
                end_col
            } else {
                cols.saturating_sub(1)
            };
            if from_col > to_col {
                continue;
            }
            for col in from_col..=to_col.min(row_cells.len().saturating_sub(1)) {
                let ch = row_cells[col].codepoint;
                if ch != '\0' {
                    out.push(ch);
                }
            }
            if row < end_row {
                out.push('\n');
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    fn handle_drop(&mut self, cx: &mut Cx, path: &str, event: &Event) -> bool {
        match event.drag_hits(cx, self.area) {
            DragHit::Drag(drag) => {
                if Self::dropped_text_payload(drag.items.as_ref()).is_none() {
                    return false;
                }
                *drag.response.lock().unwrap() = DragResponse::Copy;
                true
            }
            DragHit::Drop(drop) => {
                let Some(payload) = Self::dropped_text_payload(drop.items.as_ref()) else {
                    return false;
                };
                self.emit_paste_text(cx, path, &payload);
                self.cursor_blink_on = true;
                self.draw_bg.redraw(cx);
                true
            }
            _ => false,
        }
    }
}

impl Widget for DesktopTerminalView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_terminal();
        cx.begin_turtle(walk, self.layout);
        self.viewport_rect = cx.turtle().rect();
        self.refresh_cell_metrics(cx);

        let mut path_for_resize: Option<String> = None;
        if let Some(data) = scope.data.get_mut::<AppData>() {
            if let Some(path) = Self::terminal_path_for_widget(cx, data, self.widget_uid()) {
                self.sync_stream_from_data(cx, data, &path);
                path_for_resize = Some(path);
            }
        }
        self.update_terminal_size(cx, path_for_resize.as_deref());

        if let Some(terminal) = &self.terminal {
            let bg = terminal.default_bg;
            self.draw_bg.draw_vars.set_uniform(
                cx,
                id!(color),
                &[
                    bg.r as f32 / 255.0,
                    bg.g as f32 / 255.0,
                    bg.b as f32 / 255.0,
                    1.0,
                ],
            );
        }
        self.draw_bg.draw_abs(cx, self.viewport_rect);
        self.draw_terminal(cx);

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Timer(timer_event) = event {
            if self.cursor_blink_timer.is_timer(timer_event).is_some() {
                self.cursor_blink_on = !self.cursor_blink_on;
                self.draw_bg.redraw(cx);
            }
        }

        let path = scope
            .data
            .get::<AppData>()
            .and_then(|data| Self::terminal_path_for_widget(cx, data, self.widget_uid()));
        let Some(path) = path else {
            return;
        };

        if self.handle_drop(cx, &path, event) {
            return;
        }

        match event.hits(cx, self.area) {
            Hit::FingerDown(FingerDownEvent { abs, .. }) => {
                cx.set_key_focus(self.area);
                let pos = self.pick(abs);
                self.selection_anchor = Some(pos);
                self.selection_cursor = Some(pos);
                self.selecting = true;
                self.cursor_blink_on = true;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(FingerMoveEvent { abs, .. }) => {
                cx.set_cursor(MouseCursor::Text);
                if self.selecting {
                    self.selection_cursor = Some(self.pick(abs));
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(_) => {
                self.selecting = false;
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Text);
            }
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => {
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(e) => {
                if Self::is_clipboard_paste_shortcut(e.key_code, &e.modifiers) {
                    return;
                }
                let sends_special_key = Self::is_special_pty_key(e.key_code);
                let sends_ctrl_char = e.modifiers.control && e.key_code.to_char(false).is_some();
                if sends_special_key {
                    self.send_key_to_terminal(cx, &path, e.key_code, &e.modifiers);
                    self.cursor_blink_on = true;
                    self.draw_bg.redraw(cx);
                } else if sends_ctrl_char {
                    if let Some(ch) = e.key_code.to_char(false) {
                        self.send_text_to_terminal(cx, &path, &ch.to_string(), &e.modifiers);
                        self.cursor_blink_on = true;
                        self.draw_bg.redraw(cx);
                    }
                }
            }
            Hit::TextInput(e) => {
                let is_newline_text = matches!(e.input.as_str(), "\n" | "\r" | "\r\n");
                if !e.was_paste && is_newline_text {
                    return;
                }
                if e.was_paste {
                    self.emit_paste_text(cx, &path, &e.input);
                } else {
                    self.send_text_to_terminal(cx, &path, &e.input, &KeyModifiers::default());
                }
                self.cursor_blink_on = true;
                self.draw_bg.redraw(cx);
            }
            Hit::TextCopy(copy_event) => {
                if let Some(text) = self.selected_text() {
                    *copy_event.response.borrow_mut() = Some(text);
                }
            }
            _ => {}
        }
    }
}

impl DesktopTerminalViewRef {
    pub fn collect_terminal_input(&self, actions: &Actions) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        for item in
            actions.filter_widget_actions_cast::<DesktopTerminalViewAction>(self.widget_uid())
        {
            if let DesktopTerminalViewAction::Input { path, data } = item {
                out.push((path, data));
            }
        }
        out
    }

    pub fn resize_requested(&self, actions: &Actions) -> Option<(String, u16, u16)> {
        for item in
            actions.filter_widget_actions_cast::<DesktopTerminalViewAction>(self.widget_uid())
        {
            if let DesktopTerminalViewAction::Resize { path, cols, rows } = item {
                return Some((path, cols, rows));
            }
        }
        None
    }
}

fn map_keycode(kc: KeyCode) -> TermKeyCode {
    use makepad_terminal_core::TermKeyCode as TK;
    match kc {
        KeyCode::ReturnKey | KeyCode::NumpadEnter => TK::Return,
        KeyCode::Tab => TK::Tab,
        KeyCode::Backspace => TK::Backspace,
        KeyCode::Escape => TK::Escape,
        KeyCode::Delete => TK::Delete,
        KeyCode::ArrowUp => TK::Up,
        KeyCode::ArrowDown => TK::Down,
        KeyCode::ArrowLeft => TK::Left,
        KeyCode::ArrowRight => TK::Right,
        KeyCode::Home => TK::Home,
        KeyCode::End => TK::End,
        KeyCode::PageUp => TK::PageUp,
        KeyCode::PageDown => TK::PageDown,
        KeyCode::Insert => TK::Insert,
        KeyCode::F1 => TK::F1,
        KeyCode::F2 => TK::F2,
        KeyCode::F3 => TK::F3,
        KeyCode::F4 => TK::F4,
        KeyCode::F5 => TK::F5,
        KeyCode::F6 => TK::F6,
        KeyCode::F7 => TK::F7,
        KeyCode::F8 => TK::F8,
        KeyCode::F9 => TK::F9,
        KeyCode::F10 => TK::F10,
        KeyCode::F11 => TK::F11,
        KeyCode::F12 => TK::F12,
        _ => TK::None,
    }
}
