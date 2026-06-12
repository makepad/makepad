use crate::makepad_widgets::scroll_bars::ScrollBarsAction;
use crate::makepad_widgets::text::rasterizer::RasterizedGlyph;
use crate::{app_data::AppData, makepad_widgets::*};
use makepad_studio_protocol::hub_protocol::TerminalFramebuffer;
use makepad_terminal_core::{TermKeyCode, Terminal};
use std::collections::HashMap;

mod selection;
mod renderer;

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
        selection_color_focus: theme.color_outset_active
        selection_color_unfocus: theme.color_outset_active * 0.65
        scroll_bars: mod.widgets.ScrollBars {
            show_scroll_x: false
            show_scroll_y: true
        }
        draw_bg +: {
            color: uniform(vec4(0.0))
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.rect(0.0, 0.0, self.rect_size.x, self.rect_size.y)
                let color = vec4((theme.color_bg_container * 1.02).rgb, 0.25)
                let highlight = 0.03 * smoothstep(1.5, 0.0, self.pos.y * self.rect_size.y)
                let noise = (Math.random_2d(self.pos * 1000.0) - 0.5) * 0.005
                sdf.fill(vec4(color.rgb + highlight + noise, color.a))
                sdf.stroke(vec4(1.0, 1.0, 1.0, 0.04), 1.0)
                return sdf.result
            }
        }
        draw_text +: {
            draw_call_group: @text
            text_style: theme.font_code
        }
        draw_cell_bg +: {}
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
        pty_rows: u16,
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
    follow_output: bool,
    #[rust]
    last_requested: Option<(String, u16, u16, u16, usize)>,
    #[rust]
    last_total_lines: usize,
    #[rust]
    last_path: Option<String>,
    #[rust]
    last_enter_time: f64,
    #[live]
    selection_color_focus: Vec4f,
    #[live]
    selection_color_unfocus: Vec4f,
    #[rust]
    selection_anchor: Option<(usize, usize)>,
    #[rust]
    selection_cursor: Option<(usize, usize)>,
    #[rust]
    selecting: bool,
    #[rust]
    select_scroll_next_frame: NextFrame,
    #[rust]
    last_finger_abs: Option<Vec2d>,
    #[rust]
    last_frame: Option<TerminalFramebuffer>,
    #[rust]
    ime_pos: Option<Vec2d>,
}

impl ScriptHook for DesktopTerminalView {}

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

    fn send_viewport_request(
        &mut self,
        cx: &mut Cx,
        path: &str,
        cols: u16,
        rows: u16,
        pty_rows: u16,
        top_row: usize,
    ) {
        let request = (path.to_string(), cols, rows, pty_rows, top_row);
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
                pty_rows: request.3,
                top_row: request.4,
            },
        );
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
        self.ime_pos = Some(dvec2(self.pad_x, self.pad_y + self.cell_height));

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
            .ceil()
            .max(1.0) as u16
            + 1;
        let req_rows_usize = req_rows as usize;
        let pty_rows = ((self.viewport_rect.size.y - self.pad_y * 2.0) / cell_height)
            .floor()
            .max(1.0) as u16;

        let total_lines_for_scroll = frame.as_ref().map(Self::scrollbar_total_lines).unwrap_or(0);
        self.last_total_lines = total_lines_for_scroll;

        if self.follow_output {
            self.stick_to_bottom(cx, self.last_total_lines);
        } else {
            self.clamp_scroll_position(cx, self.last_total_lines);
        }

        let visible_top_row = (self.current_scroll_pixels() / cell_height)
            .floor()
            .max(0.0) as usize;
        let selection = self.selection_ordered();
        let (requested_top_row, requested_end_row_exclusive) =
            Self::requested_frame_range(visible_top_row, req_rows_usize, selection);
        let requested_rows = requested_end_row_exclusive
            .saturating_sub(requested_top_row)
            .max(req_rows_usize)
            .min(u16::MAX as usize) as u16;

        let frame_matches_viewport = frame
            .as_ref()
            .map(|frame| {
                let frame_end_row_exclusive = frame.top_row.saturating_add(frame.rows as usize);
                frame.cols == req_cols
                    && frame.rows >= req_rows
                    && (selection.is_none()
                        || (frame.top_row <= requested_top_row
                            && frame_end_row_exclusive >= requested_end_row_exclusive))
            })
            .unwrap_or(false);
        if frame.is_some() && !frame_matches_viewport {
            self.last_requested = None;
        }

        if let Some(path) = path.as_deref() {
            let top_row = if selection.is_some() {
                requested_top_row
            } else if self.follow_output {
                usize::MAX
            } else {
                let top = (self.current_scroll_pixels() / cell_height)
                    .floor()
                    .max(0.0) as usize;
                let max_top = self
                    .last_total_lines
                    .saturating_sub(pty_rows.max(1) as usize);
                top.min(max_top)
            };
            let rows = if selection.is_some() {
                requested_rows
            } else {
                req_rows
            };
            self.send_viewport_request(cx, path, req_cols, rows, pty_rows, top_row);
        }

        self.last_frame = frame.clone();
        if let Some(frame) = frame.as_ref() {
            let bg = Self::decode_rgb(frame.default_bg_rgb);
            self.draw_bg
                .draw_vars
                .set_uniform(cx, id!(color), &[bg.x, bg.y, bg.z, bg.w]);
            self.draw_bg.draw_abs(cx, self.unscrolled_rect);
            self.draw_framebuffer(cx, frame);
        } else {
            self.draw_bg.draw_abs(cx, self.unscrolled_rect);
        }

        let content_height = self.content_height_for_total_lines(self.last_total_lines);
        let used_height = if content_height <= self.viewport_rect.size.y + 0.1 {
            self.viewport_rect.size.y
        } else {
            content_height
        };

        cx.turtle_mut()
            .set_used(self.viewport_rect.size.x.max(1.0), used_height);
        self.scroll_bars.end(cx);
        self.area = self.scroll_bars.area();
        if path.is_some() && cx.has_key_focus(self.scroll_bars.area()) {
            if let Some(ime_pos) = self.ime_pos {
                cx.show_text_ime(self.scroll_bars.area(), ime_pos);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
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

        if self.selecting && self.select_scroll_next_frame.is_event(event).is_some() {
            self.select_scroll_next_frame = cx.new_next_frame();
            if let Some(abs) = self.last_finger_abs {
                let vp_rect = self.scroll_bars.area().clipped_rect(cx);
                let vp_top = vp_rect.pos.y;
                let vp_bottom = vp_top + vp_rect.size.y;
                let (_, cell_height) = self.cell_metrics();
                let scroll_speed = cell_height * 2.0;
                let edge_band = cell_height.max(4.0);
                let top_trigger = vp_top + edge_band;
                let bottom_trigger = vp_bottom - edge_band;

                if abs.y <= top_trigger {
                    let delta = (top_trigger - abs.y)
                        .max(cell_height * 0.25)
                        .min(scroll_speed);
                    let new_y = (self.current_scroll_pixels() - delta).max(0.0);
                    let _ = self
                        .scroll_bars
                        .set_scroll_pos_no_clip(cx, dvec2(0.0, new_y));
                } else if abs.y >= bottom_trigger {
                    let delta = (abs.y - bottom_trigger)
                        .max(cell_height * 0.25)
                        .min(scroll_speed);
                    let max = self.max_scroll_pixels_for_total_lines(self.last_total_lines);
                    let new_y = (self.current_scroll_pixels() + delta).min(max);
                    let _ = self
                        .scroll_bars
                        .set_scroll_pos_no_clip(cx, dvec2(0.0, new_y));
                }

                self.follow_output = self.is_scrolled_to_bottom(self.last_total_lines);
                self.selection_cursor = Some(self.pick(abs));
                self.draw_bg.redraw(cx);
            }
        }

        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(FingerDownEvent { abs, tap_count, .. }) => {
                cx.set_key_focus(self.scroll_bars.area());
                let pos = self.pick(abs);
                if tap_count == 2 {
                    if let Some(frame) = self.last_frame.as_ref() {
                        if let Some((start_col, end_col)) =
                            Self::word_range_at_in_frame(frame, pos.0, pos.1)
                        {
                            self.selection_anchor = Some((pos.0, start_col));
                            self.selection_cursor = Some((pos.0, end_col));
                        } else {
                            self.selection_anchor = Some(pos);
                            self.selection_cursor = Some(pos);
                        }
                    }
                    self.selecting = false;
                    self.last_finger_abs = None;
                } else {
                    self.selection_anchor = Some(pos);
                    self.selection_cursor = Some(pos);
                    self.selecting = true;
                    self.last_finger_abs = Some(abs);
                    self.select_scroll_next_frame = cx.new_next_frame();
                }
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(FingerMoveEvent { abs, .. }) => {
                cx.set_cursor(MouseCursor::Text);
                if self.selecting {
                    self.selection_cursor = Some(self.pick(abs));
                    self.last_finger_abs = Some(abs);
                    self.draw_bg.redraw(cx);
                }
            }
            Hit::FingerUp(_) => {
                self.selecting = false;
                self.last_finger_abs = None;
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Text);
            }
            Hit::KeyFocus(_) => {
                self.draw_bg.redraw(cx);
            }
            Hit::KeyFocusLost(_) => {
                cx.hide_text_ime();
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(e) => {
                if path.is_empty() {
                    return;
                }

                if Self::is_clipboard_paste_shortcut(e.key_code, &e.modifiers) {
                    return;
                }
                let is_enter =
                    e.key_code == KeyCode::ReturnKey || e.key_code == KeyCode::NumpadEnter;
                if is_enter && e.is_repeat && (e.time - self.last_enter_time) < 0.08 {
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
                    if is_enter {
                        self.last_enter_time = e.time;
                    }
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
                        self.draw_bg.redraw(cx);
                    }
                }
            }
            Hit::TextInput(e) => {
                if path.is_empty() {
                    return;
                }

                if e.replace_last {
                    return;
                }

                if e.was_paste {
                    self.emit_paste_text(cx, &path, &e.input, bracketed_paste);
                } else {
                    let filtered: String = e
                        .input
                        .chars()
                        .filter(|c| *c != '\n' && *c != '\r')
                        .collect();
                    if filtered.is_empty() {
                        return;
                    }
                    self.send_text_to_terminal(
                        cx,
                        &path,
                        &filtered,
                        &KeyModifiers::default(),
                        cursor_keys_application_mode,
                    );
                }
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

    pub fn viewport_request(&self, actions: &Actions) -> Option<(String, u16, u16, u16, usize)> {
        for item in
            actions.filter_widget_actions_cast::<DesktopTerminalViewAction>(self.widget_uid())
        {
            if let DesktopTerminalViewAction::RequestViewport {
                path,
                cols,
                rows,
                pty_rows,
                top_row,
            } = item
            {
                return Some((path, cols, rows, pty_rows, top_row));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollbar_total_lines_preserves_scrollback_for_custom_scroll_region_apps() {
        let frame = TerminalFramebuffer {
            is_tui: true,
            rows: 20,
            total_lines: 240,
            ..Default::default()
        };

        assert_eq!(DesktopTerminalView::scrollbar_total_lines(&frame), 240);
    }

    #[test]
    fn scrollbar_total_lines_keeps_plain_terminal_history() {
        let frame = TerminalFramebuffer {
            is_tui: false,
            rows: 20,
            total_lines: 75,
            ..Default::default()
        };

        assert_eq!(DesktopTerminalView::scrollbar_total_lines(&frame), 75);
    }

    #[test]
    fn requested_frame_range_expands_to_cover_selection_outside_viewport() {
        let selection = Some(((12, 4), (34, 9)));
        let (start_row, end_row_exclusive) =
            DesktopTerminalView::requested_frame_range(20, 8, selection);

        assert_eq!(start_row, 12);
        assert_eq!(end_row_exclusive, 35);
    }

    #[test]
    fn visible_frame_rows_starts_rendering_at_current_scroll_offset() {
        let frame = TerminalFramebuffer {
            top_row: 80,
            rows: 40,
            ..Default::default()
        };

        let (start_row, render_rows, origin_y) =
            DesktopTerminalView::visible_frame_rows(&frame, 955.0, 10.0, 6, 100.0)
                .expect("expected visible rows");

        assert_eq!(start_row, 15);
        assert_eq!(render_rows, 6);
        assert_eq!(origin_y, 95.0);
    }
}
