use crate::makepad_widgets::*;
use makepad_terminal_core::{Color, CursorShape, Pty, StyleFlags, TermKeyCode, Terminal};
use std::io;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawTerminalCellBg::script_shader(vm)) {
        ..mod.draw.DrawQuad
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
        color: #f00
        pixel: fn() {
            return vec4(self.color.rgb * self.color.a, self.color.a)
        }
    }

    mod.widgets.StudioTerminalBase = #(StudioTerminal::register_widget(vm))

    mod.widgets.StudioTerminal = set_type_default() do mod.widgets.StudioTerminalBase {
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
            text_style: theme.font_code
        }
        draw_selection +: {
        }
        draw_decor +: {
        }
        draw_cursor +: {
        }
    }
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
}

#[derive(Script, ScriptHook, Widget)]
pub struct StudioTerminal {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    scroll_bars: ScrollBars,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[live]
    draw_cursor: DrawTerminalCursor,
    #[live]
    draw_selection: DrawTerminalCellBg,
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

    #[rust]
    terminal: Option<Terminal>,
    #[rust]
    pty: Option<Pty>,
    #[rust]
    pty_spawn_rx: Option<Receiver<io::Result<Pty>>>,
    #[rust]
    pty_spawn_in_flight: bool,
    #[rust]
    initialized: bool,
    #[rust]
    last_size: (usize, usize),
    #[rust]
    poll_timer: Timer,
    #[rust]
    cursor_blink_timer: Timer,
    #[rust]
    cursor_blink_on: bool,
    #[rust]
    follow_output: bool,
    #[rust]
    viewport_rect: Rect,
    #[rust]
    pending_scroll_clamp: bool,
    #[rust]
    area: Area,
    #[rust]
    output_streaming: bool,
    #[rust]
    last_output_at: Option<Instant>,
    #[rust]
    cell_width: f64,
    #[rust]
    cell_height: f64,
    #[rust]
    cell_offset_y: f64,
}

impl StudioTerminal {
    const OUTPUT_QUIET_DELAY: Duration = Duration::from_millis(120);

    fn scale_channel(v: u8, factor: f64) -> u8 {
        ((v as f64 * factor).round()).clamp(0.0, 255.0) as u8
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

    fn max_scroll_rows(screen: &makepad_terminal_core::Screen) -> usize {
        screen.total_rows().saturating_sub(screen.rows())
    }

    fn current_scroll_pixels(&self) -> f64 {
        self.scroll_bars.get_scroll_pos().y.max(0.0)
    }

    fn max_scroll_pixels(&self) -> f64 {
        (self.content_height() - self.viewport_rect.size.y).max(0.0)
    }

    fn current_scroll_rows(&self) -> usize {
        let (_, cell_height) = self.cell_metrics();
        (self.scroll_bars.get_scroll_pos().y.max(0.0) / cell_height).floor() as usize
    }

    fn is_scrolled_to_bottom(&self, screen: &makepad_terminal_core::Screen) -> bool {
        let _ = screen;
        self.current_scroll_pixels() >= self.max_scroll_pixels() - 1.0
    }

    fn clamp_scroll_position(&mut self, cx: &mut Cx) {
        let y = self.current_scroll_pixels().min(self.max_scroll_pixels());
        let _ = self.scroll_bars.set_scroll_pos_no_clip(cx, dvec2(0.0, y));
    }

    fn stick_to_bottom(&mut self, cx: &mut Cx) {
        let y = self.max_scroll_pixels();
        let _ = self.scroll_bars.set_scroll_pos_no_clip(cx, dvec2(0.0, y));
        self.follow_output = true;
    }

    fn content_height(&self) -> f64 {
        let Some(terminal) = &self.terminal else {
            return self.viewport_rect.size.y.max(1.0);
        };
        let (_, cell_height) = self.cell_metrics();
        let total_rows = terminal.screen().total_rows();
        let content_rows_h = total_rows as f64 * cell_height;
        (content_rows_h + self.pad_y * 2.0).max(self.viewport_rect.size.y)
    }

    fn update_output_streaming_state(&mut self, cx: &mut Cx) {
        if !self.output_streaming {
            return;
        }
        if let Some(last) = self.last_output_at {
            if last.elapsed() >= Self::OUTPUT_QUIET_DELAY {
                self.output_streaming = false;
                self.cursor_blink_on = true;
                self.draw_bg.redraw(cx);
            }
        }
    }

    fn is_visible(&self, cx: &Cx) -> bool {
        self.area.is_valid(cx)
    }

    fn ensure_pty(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.cursor_blink_on = true;
        self.follow_output = true;
        self.terminal = Some(Terminal::new(80, 24));

        self.poll_timer = cx.start_interval(0.016);
        self.cursor_blink_timer = cx.start_interval(0.53);

        let (tx, rx) = mpsc::channel::<io::Result<Pty>>();
        self.pty_spawn_rx = Some(rx);
        self.pty_spawn_in_flight = true;

        if std::thread::Builder::new()
            .name("studio-pty-spawn".to_string())
            .spawn(move || {
                let child_env = [
                    ("COLORTERM", "truecolor"),
                    ("TERM_PROGRAM", "makepad-studio"),
                    ("TERM_PROGRAM_VERSION", "0.1"),
                ];
                let _ = tx.send(Pty::spawn(80, 24, None, &child_env));
            })
            .is_err()
        {
            self.pty_spawn_in_flight = false;
            self.pty_spawn_rx = None;
            log!("Failed to create PTY spawn thread");
        }
    }

    fn poll_pty_spawn(&mut self, cx: &mut Cx) {
        if !self.pty_spawn_in_flight {
            return;
        }
        let Some(rx) = &self.pty_spawn_rx else {
            self.pty_spawn_in_flight = false;
            return;
        };

        match rx.try_recv() {
            Ok(Ok(pty)) => {
                self.pty = Some(pty);
                self.pty_spawn_in_flight = false;
                self.pty_spawn_rx = None;

                if self.last_size.0 > 0 && self.last_size.1 > 0 {
                    if let Some(pty) = &self.pty {
                        let _ = pty.resize(self.last_size.0 as u16, self.last_size.1 as u16);
                    }
                }
                self.stick_to_bottom(cx);
                self.draw_bg.redraw(cx);
            }
            Ok(Err(e)) => {
                self.pty_spawn_in_flight = false;
                self.pty_spawn_rx = None;
                log!("Failed to spawn PTY: {}", e);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.pty_spawn_in_flight = false;
                self.pty_spawn_rx = None;
                log!("PTY spawn channel disconnected");
            }
        }
    }

    fn poll_pty_output(&mut self, cx: &mut Cx) {
        let was_at_bottom = if let Some(terminal) = &self.terminal {
            self.follow_output || self.is_scrolled_to_bottom(terminal.screen())
        } else {
            true
        };

        let Some(pty) = &self.pty else { return };
        const MAX_CHUNKS_PER_TICK: usize = 256;
        const MAX_BYTES_PER_TICK: usize = 1 << 20;

        let mut got_data = false;
        let mut scrollback_changed = false;
        let mut total_bytes = 0usize;
        let mut chunks = 0usize;
        {
            let Some(terminal) = &mut self.terminal else {
                return;
            };
            let old_scrollback = terminal.screen().scrollback_len();
            while chunks < MAX_CHUNKS_PER_TICK && total_bytes < MAX_BYTES_PER_TICK {
                let Some(data) = pty.try_read() else {
                    break;
                };
                total_bytes += data.len();
                terminal.process_bytes(&data);
                let outbound = terminal.take_outbound();
                if !outbound.is_empty() {
                    let _ = pty.write(&outbound);
                }
                got_data = true;
                chunks += 1;
            }
            if got_data {
                scrollback_changed = terminal.screen().scrollback_len() != old_scrollback;
            }
        }

        if got_data {
            self.last_output_at = Some(Instant::now());
            if !self.output_streaming || self.cursor_blink_on {
                self.output_streaming = true;
                self.cursor_blink_on = false;
            }
            if scrollback_changed {
                if was_at_bottom {
                    self.stick_to_bottom(cx);
                } else {
                    self.clamp_scroll_position(cx);
                }
            }
            self.draw_bg.redraw(cx);
        }
    }

    fn send_key_to_pty(&self, key_code: KeyCode, modifiers: &KeyModifiers) {
        let Some(pty) = &self.pty else { return };
        let Some(terminal) = &self.terminal else {
            return;
        };

        let tc_key = map_keycode(key_code);
        if let Some(bytes) = terminal.encode_key(
            tc_key,
            "",
            modifiers.shift,
            modifiers.control,
            modifiers.alt,
        ) {
            let _ = pty.write(&bytes);
        }
    }

    fn send_text_to_pty(&self, text: &str, modifiers: &KeyModifiers) {
        let Some(pty) = &self.pty else { return };

        if modifiers.control {
            let Some(terminal) = &self.terminal else {
                return;
            };
            if let Some(bytes) = terminal.encode_key(
                TermKeyCode::None,
                text,
                modifiers.shift,
                true,
                modifiers.alt,
            ) {
                let _ = pty.write(&bytes);
            }
        } else if modifiers.alt {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(text.as_bytes());
            let _ = pty.write(&bytes);
        } else {
            let _ = pty.write(text.as_bytes());
        }
    }

    fn draw_terminal(&mut self, cx: &mut Cx2d) {
        let Some(terminal) = &self.terminal else {
            return;
        };
        let screen = terminal.screen();
        let cols = screen.cols();
        let rows = screen.rows();

        let (cell_width, cell_height) = self.cell_metrics();
        let origin_x = self.viewport_rect.pos.x + self.pad_x;
        let origin_y = self.viewport_rect.pos.y + self.pad_y;
        let scroll_y = self.current_scroll_pixels();

        let max_scroll_rows = Self::max_scroll_rows(screen);
        let top_row = self.current_scroll_rows().min(max_scroll_rows);

        let palette = &terminal.palette.colors;
        let default_fg = terminal.default_fg;
        let default_bg = terminal.default_bg;
        let default_cursor = terminal.cursor_color.unwrap_or(default_fg);
        let blank_cell = makepad_terminal_core::Cell::default();

        self.draw_cursor.color = vec4(
            default_cursor.r as f32 / 255.0,
            default_cursor.g as f32 / 255.0,
            default_cursor.b as f32 / 255.0,
            1.0,
        );

        let resolve_style = |style: &makepad_terminal_core::Style| {
            let mut fg_src = style.fg;
            let mut bg_src = style.bg;
            let flags = style.flags;
            if flags.has(StyleFlags::INVERSE) {
                std::mem::swap(&mut fg_src, &mut bg_src);
            }

            let mut fg_color = fg_src.resolve(palette, default_fg);
            let bg_color = bg_src.resolve(palette, default_bg);

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
            let bg_r = bg_color.r;
            let bg_g = bg_color.g;
            let bg_b = bg_color.b;

            if flags.has(StyleFlags::FAINT) {
                fg_r = Self::scale_channel(fg_r, self.faint_factor);
                fg_g = Self::scale_channel(fg_g, self.faint_factor);
                fg_b = Self::scale_channel(fg_b, self.faint_factor);
            }
            if flags.has(StyleFlags::INVISIBLE) {
                fg_r = bg_r;
                fg_g = bg_g;
                fg_b = bg_b;
            }

            (flags, fg_r, fg_g, fg_b, bg_r, bg_g, bg_b)
        };

        // Predefine terminal layer order (like code_editor):
        // background-cells -> cursor -> text -> decorations.
        self.draw_selection.new_draw_call(cx);
        self.draw_cursor.new_draw_call(cx);
        self.draw_text.new_draw_call(cx);
        self.draw_decor.new_draw_call(cx);

        // Cursor is emitted to its own predefined layer so it remains behind text.
        if terminal.modes.cursor_visible && self.cursor_blink_on && !self.output_streaming {
            let cursor = &screen.cursor;
            let cursor_virtual_y = screen.scrollback_len() + cursor.y;
            let cursor_content_y = cursor_virtual_y as f64 * cell_height;
            if !(cursor_content_y + cell_height < scroll_y
                || cursor_content_y > scroll_y + self.viewport_rect.size.y)
            {
                let cx_x = origin_x + cursor.x as f64 * cell_width;
                let cx_y = origin_y + cursor_content_y + self.cursor_y_offset;

                let cursor_rect = match cursor.shape {
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
                };

                self.draw_cursor.draw_abs(cx, cursor_rect);
            }
        }

        // Draw one extra row to handle partial-row viewport offsets near edges.
        // We emit interleaved while iterating cells; each draw appends to its predefined layer.
        let total_draw_rows = rows.saturating_add(1);
        for row in 0..total_draw_rows {
            let virtual_row = top_row + row;
            let row_slice = screen.row_slice_virtual(virtual_row);
            for col in 0..cols {
                let cell = row_slice.and_then(|r| r.get(col)).unwrap_or(&blank_cell);
                let (flags, fg_r, fg_g, fg_b, bg_r, bg_g, bg_b) = resolve_style(&cell.style);
                let x = origin_x + col as f64 * cell_width;
                let y = origin_y + virtual_row as f64 * cell_height;

                if bg_r != default_bg.r || bg_g != default_bg.g || bg_b != default_bg.b {
                    self.draw_selection.color = vec4(
                        bg_r as f32 / 255.0,
                        bg_g as f32 / 255.0,
                        bg_b as f32 / 255.0,
                        1.0,
                    );
                    self.draw_selection.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, y),
                            size: dvec2(cell_width, cell_height),
                        },
                    );
                }

                let blink_hidden = flags.has(StyleFlags::BLINK) && !self.cursor_blink_on;
                let ch = cell.codepoint;
                if ch == ' ' || ch == '\0' || blink_hidden || flags.has(StyleFlags::INVISIBLE) {
                } else {
                    let mut s = [0u8; 4];
                    let text = ch.encode_utf8(&mut s);
                    self.draw_text.color = vec4(
                        fg_r as f32 / 255.0,
                        fg_g as f32 / 255.0,
                        fg_b as f32 / 255.0,
                        1.0,
                    );
                    self.draw_text
                        .draw_abs(cx, dvec2(x, y + self.cell_offset_y + self.text_y_offset), text);
                    if flags.has(StyleFlags::BOLD) {
                        self.draw_text.draw_abs(
                            cx,
                            dvec2(x + 0.6, y + self.cell_offset_y + self.text_y_offset),
                            text,
                        );
                    }
                }

                let underline = flags.underline();
                let strike = flags.has(StyleFlags::STRIKETHROUGH);
                let overline = flags.has(StyleFlags::OVERLINE);
                if underline == 0 && !strike && !overline {
                    continue;
                }
                self.draw_decor.color = vec4(
                    fg_r as f32 / 255.0,
                    fg_g as f32 / 255.0,
                    fg_b as f32 / 255.0,
                    1.0,
                );
                if underline != 0 {
                    let uy = y + cell_height - 2.0;
                    self.draw_decor.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, uy),
                            size: dvec2(cell_width, 1.0),
                        },
                    );
                    if underline == 2 {
                        self.draw_decor.draw_abs(
                            cx,
                            Rect {
                                pos: dvec2(x, uy - 2.0),
                                size: dvec2(cell_width, 1.0),
                            },
                        );
                    }
                }
                if strike {
                    self.draw_decor.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, y + cell_height * 0.5),
                            size: dvec2(cell_width, 1.0),
                        },
                    );
                }
                if overline {
                    self.draw_decor.draw_abs(
                        cx,
                        Rect {
                            pos: dvec2(x, y + 1.0),
                            size: dvec2(cell_width, 1.0),
                        },
                    );
                }
            }
        }
    }

    fn update_terminal_size(&mut self, _cx: &mut Cx2d) {
        let rect = self.viewport_rect;
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return;
        }

        let (cell_width, cell_height) = self.cell_metrics();

        let cols = ((rect.size.x - self.pad_x * 2.0) / cell_width)
            .floor()
            .max(1.0) as usize;
        let rows = ((rect.size.y - self.pad_y * 2.0) / cell_height)
            .floor()
            .max(1.0) as usize;

        if (cols, rows) != self.last_size && cols > 0 && rows > 0 {
            self.last_size = (cols, rows);
            if let Some(terminal) = &mut self.terminal {
                terminal.resize(cols, rows);
            }
            if let Some(pty) = &self.pty {
                let _ = pty.resize(cols as u16, rows as u16);
            }
            self.pending_scroll_clamp = true;
        }
    }
}

impl Widget for StudioTerminal {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.scroll_bars.begin(cx, walk, Layout::default());
        self.viewport_rect = cx.turtle().rect();
        self.refresh_cell_metrics(cx);
        self.update_terminal_size(cx);
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
        self.draw_bg.draw_abs(cx, cx.turtle().rect_unscrolled());
        self.draw_terminal(cx);
        cx.turtle_mut()
            .set_used(self.viewport_rect.size.x.max(1.0), self.content_height());
        self.scroll_bars.end(cx);
        self.area = self.scroll_bars.area();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let visible = self.is_visible(cx);

        if visible {
            self.ensure_pty(cx);

            if self.scroll_bars.handle_event(cx, event, scope).len() > 0 {
                if let Some(terminal) = &self.terminal {
                    self.follow_output = self.is_scrolled_to_bottom(terminal.screen());
                }
                self.draw_bg.redraw(cx);
            }
        }

        match event {
            Event::Timer(te) => {
                if self.poll_timer.is_timer(te).is_some() {
                    if !visible {
                        return;
                    }
                    if self.pending_scroll_clamp {
                        if self.follow_output {
                            self.stick_to_bottom(cx);
                        } else {
                            self.clamp_scroll_position(cx);
                        }
                        self.pending_scroll_clamp = false;
                    }
                    self.poll_pty_spawn(cx);
                    self.poll_pty_output(cx);
                    self.update_output_streaming_state(cx);
                }
                if self.cursor_blink_timer.is_timer(te).is_some() {
                    if !visible {
                        return;
                    }
                    if self.output_streaming {
                        if self.cursor_blink_on {
                            self.cursor_blink_on = false;
                            self.draw_bg.redraw(cx);
                        }
                    } else {
                        self.cursor_blink_on = !self.cursor_blink_on;
                        self.draw_bg.redraw(cx);
                    }
                }
            }
            _ => {}
        }

        if !visible {
            return;
        }

        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(_) => {
                cx.set_key_focus(self.scroll_bars.area());
                self.cursor_blink_on = true;
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(e) => {
                self.cursor_blink_on = true;
                match e.key_code {
                    KeyCode::ReturnKey
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
                    | KeyCode::F12 => self.send_key_to_pty(e.key_code, &e.modifiers),
                    _ => {
                        if e.modifiers.control {
                            if let Some(c) = e.key_code.to_char(false) {
                                let s = c.to_string();
                                self.send_text_to_pty(&s, &e.modifiers);
                            }
                        }
                    }
                }
            }
            Hit::TextInput(e) => {
                if !e.was_paste {
                    self.send_text_to_pty(&e.input, &KeyModifiers::default());
                } else {
                    let bracketed = self
                        .terminal
                        .as_ref()
                        .map(|t| t.modes.bracketed_paste)
                        .unwrap_or(false);
                    if let Some(pty) = &self.pty {
                        if bracketed {
                            let _ = pty.write(b"\x1b[200~");
                            let _ = pty.write(e.input.as_bytes());
                            let _ = pty.write(b"\x1b[201~");
                        } else {
                            let _ = pty.write(e.input.as_bytes());
                        }
                    }
                }
            }
            Hit::TextCopy(_) => {}
            _ => {}
        }
    }
}

fn map_keycode(kc: KeyCode) -> TermKeyCode {
    use makepad_terminal_core::TermKeyCode as TK;
    match kc {
        KeyCode::ReturnKey => TK::Return,
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
