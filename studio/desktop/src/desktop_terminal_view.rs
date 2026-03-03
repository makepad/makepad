use crate::makepad_widgets::scroll_bars::ScrollBarsAction;
use crate::makepad_widgets::text::geom::Point;
use crate::makepad_widgets::text::rasterizer::RasterizedGlyph;
use crate::{app_data::AppData, makepad_widgets::*};
use makepad_studio_protocol::hub_protocol::TerminalFramebuffer;
use makepad_terminal_core::{TermKeyCode, Terminal};
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
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: false
            show_scroll_y: true
        }
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
    RequestViewport {
        path: String,
        cols: u16,
        rows: u16,
        top_row: usize,
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
    #[live]
    scroll_bars: ScrollBars,
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
    #[rust]
    area: Area,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    unscrolled_rect: Rect,
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
    follow_output: bool,
    #[rust]
    last_requested: Option<(String, u16, u16, usize)>,
    #[rust]
    last_total_lines: usize,
    #[rust]
    last_path: Option<String>,
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
        let tabs = &data.mounts.get(mount)?.terminal_tab_to_path;
        let path = cx.widget_tree().path_to(widget_uid);
        for node in path.iter().rev() {
            if let Some(terminal_path) = tabs.get(node) {
                return Some(terminal_path.clone());
            }
        }
        None
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

    fn current_scroll_pixels(&self) -> f64 {
        self.scroll_bars.get_scroll_pos().y.max(0.0)
    }

    fn content_height_for_total_lines(&self, total_lines: usize) -> f64 {
        let (_, cell_height) = self.cell_metrics();
        let content_rows_h = total_lines.max(1) as f64 * cell_height;
        (content_rows_h + self.pad_y * 2.0).max(self.viewport_rect.size.y)
    }

    fn max_scroll_pixels_for_total_lines(&self, total_lines: usize) -> f64 {
        (self.content_height_for_total_lines(total_lines) - self.viewport_rect.size.y).max(0.0)
    }

    fn is_scrolled_to_bottom(&self, total_lines: usize) -> bool {
        self.current_scroll_pixels() >= self.max_scroll_pixels_for_total_lines(total_lines) - 1.0
    }

    fn clamp_scroll_position(&mut self, cx: &mut Cx, total_lines: usize) {
        let y = self
            .current_scroll_pixels()
            .min(self.max_scroll_pixels_for_total_lines(total_lines));
        let _ = self.scroll_bars.set_scroll_pos_no_clip(cx, dvec2(0.0, y));
    }

    fn stick_to_bottom(&mut self, cx: &mut Cx, total_lines: usize) {
        let y = self.max_scroll_pixels_for_total_lines(total_lines);
        let _ = self.scroll_bars.set_scroll_pos_no_clip(cx, dvec2(0.0, y));
        self.follow_output = true;
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

    fn decode_cell(frame: &TerminalFramebuffer, row: usize, col: usize) -> Option<(char, Vec4f)> {
        let cols = frame.cols as usize;
        let rows = frame.rows as usize;
        if row >= rows || col >= cols {
            return None;
        }
        let idx = (row * cols + col) * 7;
        if idx + 6 >= frame.cells.len() {
            return None;
        }
        let codepoint = u32::from_le_bytes([
            frame.cells[idx],
            frame.cells[idx + 1],
            frame.cells[idx + 2],
            frame.cells[idx + 3],
        ]);
        let ch = char::from_u32(codepoint).unwrap_or(' ');
        let bg = vec4(
            frame.cells[idx + 4] as f32 / 255.0,
            frame.cells[idx + 5] as f32 / 255.0,
            frame.cells[idx + 6] as f32 / 255.0,
            1.0,
        );
        Some((if ch == '\0' { ' ' } else { ch }, bg))
    }

    fn decode_rgb(rgb: u32) -> Vec4f {
        vec4(
            ((rgb >> 16) & 0xff) as f32 / 255.0,
            ((rgb >> 8) & 0xff) as f32 / 255.0,
            (rgb & 0xff) as f32 / 255.0,
            1.0,
        )
    }

    fn send_viewport_request(
        &mut self,
        cx: &mut Cx,
        path: &str,
        cols: u16,
        rows: u16,
        top_row: usize,
    ) {
        let request = (path.to_string(), cols, rows, top_row);
        if self.last_requested.as_ref() == Some(&request) {
            return;
        }
        self.last_requested = Some(request.clone());
        cx.widget_action(
            self.widget_uid(),
            DesktopTerminalViewAction::RequestViewport {
                path: request.0,
                cols: request.1,
                rows: request.2,
                top_row: request.3,
            },
        );
    }

    fn draw_framebuffer(&mut self, cx: &mut Cx2d, frame: &TerminalFramebuffer) {
        let cols = frame.cols as usize;
        let rows = frame.rows as usize;
        if cols == 0 || rows == 0 {
            return;
        }

        let (cell_width, cell_height) = self.cell_metrics();

        // Use unscrolled_rect (actual screen coordinates) for all positioning.
        // This decouples rendering from the scroll offset entirely, preventing
        // sub-pixel jitter when the scroll position changes during resize.
        let screen_top = self.unscrolled_rect.pos.y + self.pad_y;
        let screen_bottom = self.unscrolled_rect.pos.y + self.unscrolled_rect.size.y - self.pad_y;
        let usable_height = (screen_bottom - screen_top).max(0.0);
        let max_visible_rows = (usable_height / cell_height).ceil().max(1.0) as usize + 1;
        let render_rows = rows.min(max_visible_rows);

        // Compute the screen-space Y origin and the first frame row to render.
        // For follow_output when screen is full: bottom-align (last row flush with viewport bottom).
        // Otherwise (not full, or scrolled up): top-align (first row at viewport top).
        let is_full_screen = frame.total_lines >= frame.rows as usize;
        let (origin_y, start_row) = if self.follow_output && is_full_screen {
            let grid_height = render_rows as f64 * cell_height;
            let y = screen_bottom - grid_height;
            let sr = rows.saturating_sub(render_rows);
            (y, sr)
        } else {
            // When top-aligned, we need to apply the sub-pixel scroll offset
            // so that scrolling is smooth and doesn't snap to cell boundaries.
            let scroll_y = self.current_scroll_pixels();
            let sub_pixel_y = scroll_y % cell_height;
            (screen_top - sub_pixel_y, 0)
        };

        let origin_x = self.unscrolled_rect.pos.x + self.pad_x;
        let default_bg = Self::decode_rgb(frame.default_bg_rgb);
        let default_fg = Self::decode_rgb(frame.default_fg_rgb);
        let has_focus = cx.has_key_focus(self.scroll_bars.area());

        self.draw_cell_bg.new_draw_call(cx);
        self.draw_cursor.new_draw_call(cx);
        self.draw_text.new_draw_call(cx);
        self.draw_decor.new_draw_call(cx);
        self.draw_text.begin_many_instances(cx);
        self.invalidate_glyph_cache_if_needed(cx);

        for i in 0..render_rows {
            let frame_row = start_row + i;
            let y = origin_y + i as f64 * cell_height;
            for col in 0..cols {
                let Some((ch, bg_color)) = Self::decode_cell(frame, frame_row, col) else {
                    continue;
                };
                let x = origin_x + col as f64 * cell_width;

                if bg_color != default_bg {
                    self.draw_cell_bg.color = bg_color;
                    self.draw_cell_bg.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, y),
                            size: dvec2(cell_width, cell_height),
                        },
                    );
                }

                if ch != ' ' {
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
                            default_fg,
                        );
                    } else {
                        let mut s = [0u8; 4];
                        let text = ch.encode_utf8(&mut s);
                        self.draw_text.color = default_fg;
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

        if self.cursor_blink_on && frame.cursor_visible && frame.cursor_row >= 0 {
            let cursor_row = frame.cursor_row as usize;
            // Map cursor's frame row to our render range
            if cursor_row >= start_row && cursor_row < start_row + render_rows {
                let visible_row = cursor_row - start_row;
                let cursor_col = (frame.cursor_col as usize).min(cols.saturating_sub(1));
                let cx_x = origin_x + cursor_col as f64 * cell_width;
                let cx_y = origin_y
                    + visible_row as f64 * cell_height
                    + self.cursor_y_offset;
                self.draw_cursor.focus = if has_focus { 1.0 } else { 0.0 };
                let cursor_rect = if has_focus {
                    Rect {
                        pos: dvec2(cx_x, cx_y),
                        size: dvec2(2.0, cell_height),
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

    fn encode_key(
        &self,
        key_code: KeyCode,
        text: &str,
        mods: &KeyModifiers,
        cursor_keys_application_mode: bool,
    ) -> Option<Vec<u8>> {
        let key = map_keycode(key_code);
        if key == TermKeyCode::None && text.is_empty() {
            return None;
        }
        let mut encoder = Terminal::new(1, 1);
        encoder.modes.cursor_keys = cursor_keys_application_mode;
        encoder.encode_key(key, text, mods.shift, mods.control, mods.alt)
    }

    fn send_key_to_terminal(
        &mut self,
        cx: &mut Cx,
        path: &str,
        key_code: KeyCode,
        mods: &KeyModifiers,
        cursor_keys_application_mode: bool,
    ) {
        if let Some(bytes) = self.encode_key(key_code, "", mods, cursor_keys_application_mode) {
            self.emit_input_bytes(cx, path, bytes);
        }
    }

    fn send_text_to_terminal(
        &mut self,
        cx: &mut Cx,
        path: &str,
        text: &str,
        mods: &KeyModifiers,
        cursor_keys_application_mode: bool,
    ) {
        if let Some(bytes) =
            self.encode_key(KeyCode::Unknown, text, mods, cursor_keys_application_mode)
        {
            self.emit_input_bytes(cx, path, bytes);
        }
    }

    fn emit_paste_text(&mut self, cx: &mut Cx, path: &str, text: &str, bracketed: bool) {
        if text.is_empty() {
            return;
        }
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

    fn is_user_scroll_event(event: &Event) -> bool {
        matches!(
            event,
            Event::Scroll(_)
                | Event::MouseDown(_)
                | Event::MouseMove(_)
                | Event::MouseUp(_)
                | Event::TouchUpdate(_)
        )
    }

    fn handle_drop(
        &mut self,
        cx: &mut Cx,
        path: &str,
        event: &Event,
        bracketed_paste: bool,
    ) -> bool {
        match event.drag_hits(cx, self.scroll_bars.area()) {
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
                self.emit_paste_text(cx, path, &payload, bracketed_paste);
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
        self.scroll_bars.begin(cx, walk, self.layout);
        self.viewport_rect = cx.turtle().rect();
        self.unscrolled_rect = cx.turtle().rect_unscrolled();
        self.refresh_cell_metrics(cx);

        let path = scope
            .data
            .get::<AppData>()
            .and_then(|data| Self::terminal_path_for_widget(cx, data, self.widget_uid()));

        if path.as_deref() != self.last_path.as_deref() {
            self.last_requested = None;
            self.follow_output = true;
            self.last_path = path.clone();
        }

        let frame = path.as_deref().and_then(|path| {
            scope
                .data
                .get::<AppData>()
                .and_then(|data| data.terminal_framebuffer_by_path.get(path).cloned())
        });

        let (cell_width, cell_height) = self.cell_metrics();
        let req_cols = ((self.viewport_rect.size.x - self.pad_x * 2.0) / cell_width)
            .floor()
            .max(1.0) as u16;
        let req_rows = ((self.viewport_rect.size.y - self.pad_y * 2.0) / cell_height)
            .floor()
            .max(1.0) as u16;

        let frame_matches_viewport = frame
            .as_ref()
            .map(|frame| frame.cols == req_cols && frame.rows == req_rows)
            .unwrap_or(false);
        if frame.is_some() && !frame_matches_viewport {
            // During rapid window drags we can briefly receive older-size
            // framebuffers. Invalidate request dedupe so we force-refresh to
            // the current viewport dimensions.
            self.last_requested = None;
        }

        let total_lines_for_scroll = frame
            .as_ref()
            .map(|frame| frame.total_lines)
            .unwrap_or(req_rows as usize);
        self.last_total_lines = total_lines_for_scroll.max(req_rows as usize);
        if self.follow_output {
            self.stick_to_bottom(cx, self.last_total_lines);
        } else {
            self.clamp_scroll_position(cx, self.last_total_lines);
        }

        if let Some(path) = path.as_deref() {
            let top_row = if self.follow_output {
                usize::MAX
            } else {
                let top = (self.current_scroll_pixels() / cell_height)
                    .floor()
                    .max(0.0) as usize;
                let max_top = self
                    .last_total_lines
                    .saturating_sub(req_rows.max(1) as usize);
                top.min(max_top)
            };
            self.send_viewport_request(cx, path, req_cols, req_rows, top_row);
        }

        if let Some(frame) = frame.as_ref() {
            let bg = Self::decode_rgb(frame.default_bg_rgb);
            self.draw_bg
                .draw_vars
                .set_uniform(cx, id!(color), &[bg.x, bg.y, bg.z, bg.w]);
            self.draw_bg.draw_abs(cx, self.unscrolled_rect);
            self.draw_framebuffer(cx, frame);
            self.last_total_lines = frame.total_lines.max(frame.rows as usize);
        } else {
            self.draw_bg.draw_abs(cx, self.unscrolled_rect);
        }

        let content_height = self.content_height_for_total_lines(self.last_total_lines);
        cx.turtle_mut()
            .set_used(self.viewport_rect.size.x.max(1.0), content_height);
        self.scroll_bars.end(cx);
        self.area = self.scroll_bars.area();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::Timer(timer_event) = event {
            if self.cursor_blink_timer.is_timer(timer_event).is_some() {
                self.cursor_blink_on = !self.cursor_blink_on;
                self.draw_bg.redraw(cx);
            }
        }

        let (path, frame) = scope
            .data
            .get::<AppData>()
            .and_then(|data| {
                Self::terminal_path_for_widget(cx, data, self.widget_uid()).map(|path| {
                    let frame = data.terminal_framebuffer_by_path.get(&path).cloned();
                    (path, frame)
                })
            })
            .unwrap_or_else(|| (String::new(), None));

        let scroll_actions = self.scroll_bars.handle_event(cx, event, scope);
        if !scroll_actions.is_empty() {
            let user_scroll_event = Self::is_user_scroll_event(event);
            if user_scroll_event
                && scroll_actions
                    .iter()
                    .any(|action| matches!(action, ScrollBarsAction::ScrollY(_)))
            {
                self.follow_output = self.is_scrolled_to_bottom(self.last_total_lines);
            }
            if user_scroll_event {
                self.last_requested = None;
            }
            self.draw_bg.redraw(cx);
        }

        let cursor_keys_application_mode = frame
            .as_ref()
            .map(|frame| frame.cursor_keys_application_mode)
            .unwrap_or(false);
        let bracketed_paste = frame
            .as_ref()
            .map(|frame| frame.bracketed_paste)
            .unwrap_or(false);

        if !path.is_empty() && self.handle_drop(cx, &path, event, bracketed_paste) {
            return;
        }

        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(_) => {
                cx.set_key_focus(self.scroll_bars.area());
                self.cursor_blink_on = true;
                self.draw_bg.redraw(cx);
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) | Hit::FingerMove(_) => {
                cx.set_cursor(MouseCursor::Text);
            }
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => {
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(e) => {
                if path.is_empty() {
                    return;
                }
                if Self::is_clipboard_paste_shortcut(e.key_code, &e.modifiers) {
                    return;
                }
                let sends_special_key = Self::is_special_pty_key(e.key_code);
                let sends_ctrl_char = e.modifiers.control && e.key_code.to_char(false).is_some();
                if sends_special_key {
                    self.send_key_to_terminal(
                        cx,
                        &path,
                        e.key_code,
                        &e.modifiers,
                        cursor_keys_application_mode,
                    );
                    self.cursor_blink_on = true;
                    self.draw_bg.redraw(cx);
                } else if sends_ctrl_char {
                    if let Some(ch) = e.key_code.to_char(false) {
                        self.send_text_to_terminal(
                            cx,
                            &path,
                            &ch.to_string(),
                            &e.modifiers,
                            cursor_keys_application_mode,
                        );
                        self.cursor_blink_on = true;
                        self.draw_bg.redraw(cx);
                    }
                }
            }
            Hit::TextInput(e) => {
                if path.is_empty() {
                    return;
                }
                let is_newline_text = matches!(e.input.as_str(), "\n" | "\r" | "\r\n");
                if !e.was_paste && is_newline_text {
                    return;
                }
                if e.was_paste {
                    self.emit_paste_text(cx, &path, &e.input, bracketed_paste);
                } else {
                    self.send_text_to_terminal(
                        cx,
                        &path,
                        &e.input,
                        &KeyModifiers::default(),
                        cursor_keys_application_mode,
                    );
                }
                self.cursor_blink_on = true;
                self.draw_bg.redraw(cx);
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

    pub fn viewport_request(&self, actions: &Actions) -> Option<(String, u16, u16, usize)> {
        for item in
            actions.filter_widget_actions_cast::<DesktopTerminalViewAction>(self.widget_uid())
        {
            if let DesktopTerminalViewAction::RequestViewport {
                path,
                cols,
                rows,
                top_row,
            } = item
            {
                return Some((path, cols, rows, top_row));
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
