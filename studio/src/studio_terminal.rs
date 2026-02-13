use crate::makepad_code_editor::draw_selection::DrawSelection;
use crate::makepad_widgets::*;
use makepad_terminal_core::{Color, CursorShape, Pty, StyleFlags, TermKeyCode, Terminal};
use std::collections::VecDeque;
use std::io;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

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
        draw_cell_bg +: {
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
    #[live]
    color_unfocused: Vec4f,
    #[live]
    focus: f32,
    #[live(1.0)]
    border_width: f32,
}

#[derive(Script, Widget)]
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
    draw_cell_bg: DrawTerminalCellBg,
    #[live]
    draw_selection: DrawSelection,
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
    unscrolled_rect: Rect,
    #[rust]
    pending_scroll_clamp: bool,
    #[rust]
    area: Area,
    #[rust]
    output_streaming: bool,
    #[rust]
    pending_streaming_ticks: u8,
    #[rust]
    pending_sync_redraw: bool,
    #[rust]
    enter_coalesce_deadline: Option<Instant>,
    #[rust]
    enter_coalesce_pending_redraw: bool,
    #[rust]
    enter_prompt_cursor_x: Option<usize>,
    #[rust]
    enter_submit_cursor_x: Option<usize>,
    #[rust]
    enter_submit_virtual_row: Option<usize>,
    #[rust]
    enter_submit_pending: usize,
    #[rust]
    pty_input_backlog: VecDeque<u8>,
    #[rust]
    last_output_at: Option<Instant>,
    #[rust]
    last_input_at: Option<Instant>,
    #[rust]
    cell_width: f64,
    #[rust]
    cell_height: f64,
    #[rust]
    cell_offset_y: f64,

    // Selection state
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
}

impl ScriptHook for StudioTerminal {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.ensure_pty(cx);
        });
    }
}

impl StudioTerminal {
    const OUTPUT_QUIET_DELAY: Duration = Duration::from_millis(120);
    const LOCAL_ECHO_GRACE: Duration = Duration::from_millis(80);
    const STREAMING_START_TICKS: u8 = 2;
    const STREAMING_START_CHUNKS: usize = 2;
    const STREAMING_START_BYTES: usize = 1024;
    const ENTER_COALESCE_DELAY: Duration = Duration::from_millis(24);

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
                self.pending_streaming_ticks = 0;
                self.cursor_blink_on = true;
                self.draw_bg.redraw(cx);
            }
        }
    }

    fn enter_waiting_for_local_prompt(&self, now: Instant) -> bool {
        self.enter_coalesce_deadline.is_some()
            && self.enter_submit_pending > 0
            && self
                .last_input_at
                .map(|last_input| now.duration_since(last_input) <= Self::LOCAL_ECHO_GRACE)
                .unwrap_or(false)
    }

    fn update_enter_coalesce_state(&mut self, cx: &mut Cx) {
        if let Some(deadline) = self.enter_coalesce_deadline {
            let now = Instant::now();
            if now >= deadline {
                if self.enter_waiting_for_local_prompt(now) {
                    self.enter_coalesce_deadline = Some(now + Self::ENTER_COALESCE_DELAY);
                    return;
                }
                self.enter_coalesce_deadline = None;
                self.enter_submit_cursor_x = None;
                self.enter_submit_virtual_row = None;
                self.enter_submit_pending = 0;
                if self.enter_coalesce_pending_redraw {
                    self.enter_coalesce_pending_redraw = false;
                    if self.pending_scroll_clamp {
                        if self.follow_output {
                            self.stick_to_bottom(cx);
                        } else {
                            self.clamp_scroll_position(cx);
                        }
                        self.pending_scroll_clamp = false;
                    }
                    self.draw_bg.redraw(cx);
                }
            }
        }
    }

    fn start_enter_coalesce_cycle(&mut self) {
        if let Some(terminal) = &self.terminal {
            let screen = terminal.screen();
            let target_x = self.enter_prompt_cursor_x.unwrap_or(screen.cursor.x);
            self.enter_submit_cursor_x = Some(target_x);
            self.enter_submit_virtual_row = Some(screen.scrollback_len() + screen.cursor.y);
        } else {
            self.enter_submit_cursor_x = None;
            self.enter_submit_virtual_row = None;
        }
        self.enter_coalesce_deadline = Some(Instant::now() + Self::ENTER_COALESCE_DELAY);
        self.enter_coalesce_pending_redraw = false;
    }

    fn note_return_submit(&mut self) {
        self.enter_submit_pending = self.enter_submit_pending.saturating_add(1);
        if self.enter_coalesce_deadline.is_none() {
            self.start_enter_coalesce_cycle();
        } else {
            self.enter_coalesce_deadline = Some(Instant::now() + Self::ENTER_COALESCE_DELAY);
        }
    }

    fn note_local_input(&mut self, cx: &mut Cx) {
        self.last_input_at = Some(Instant::now());
        self.pending_streaming_ticks = 0;
        self.enter_coalesce_deadline = None;
        self.enter_coalesce_pending_redraw = false;
        self.enter_submit_cursor_x = None;
        self.enter_submit_virtual_row = None;
        self.enter_submit_pending = 0;
        self.clear_selection();
        let mut redraw = false;
        if self.output_streaming {
            self.output_streaming = false;
            redraw = true;
        }
        if !self.cursor_blink_on {
            self.cursor_blink_on = true;
            redraw = true;
        }
        if redraw {
            self.draw_bg.redraw(cx);
        }
    }

    fn note_local_input_preserve_enter_coalesce(&mut self, cx: &mut Cx) {
        self.last_input_at = Some(Instant::now());
        self.pending_streaming_ticks = 0;
        self.clear_selection();
        let mut redraw = false;
        if self.output_streaming {
            self.output_streaming = false;
            redraw = true;
        }
        if !self.cursor_blink_on {
            self.cursor_blink_on = true;
            redraw = true;
        }
        if redraw {
            self.draw_bg.redraw(cx);
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
        let poll_started_at = Instant::now();
        let strict_enter_coalesce = self.enter_waiting_for_local_prompt(poll_started_at);

        let Some(pty) = &self.pty else { return };
        const MAX_CHUNKS_PER_TICK: usize = 256;
        const MAX_BYTES_PER_TICK: usize = 1 << 20;

        let mut got_data = false;
        let mut scrollback_changed = false;
        let mut total_bytes = 0usize;
        let mut chunks = 0usize;
        let mut only_crlf_output = true;
        let mut prompt_line_settled_in_parse = false;
        let cursor_x: usize;
        let cursor_virtual_row: usize;
        let synchronized_update;
        let submit_cursor_x = self.enter_submit_cursor_x;
        let submit_virtual_row = self.enter_submit_virtual_row;
        let mut pty_input_backlog = std::mem::take(&mut self.pty_input_backlog);
        let (old_scrollback_len, new_scrollback_len) = {
            let Some(terminal) = &mut self.terminal else {
                return;
            };
            let old_scrollback = terminal.screen().scrollback_len();
            while chunks < MAX_CHUNKS_PER_TICK && total_bytes < MAX_BYTES_PER_TICK {
                if pty_input_backlog.is_empty() {
                    let Some(data) = pty.try_read() else {
                        break;
                    };
                    pty_input_backlog.extend(data);
                    chunks += 1;
                }

                if strict_enter_coalesce {
                    let Some(byte) = pty_input_backlog.pop_front() else {
                        continue;
                    };
                    if byte != b'\r' && byte != b'\n' {
                        only_crlf_output = false;
                    }
                    total_bytes += 1;
                    terminal.process_bytes(&[byte]);
                    let outbound = terminal.take_outbound();
                    if !outbound.is_empty() {
                        let _ = pty.write(&outbound);
                    }
                    got_data = true;

                    if let (Some(submit_row), Some(submit_x)) =
                        (submit_virtual_row, submit_cursor_x)
                    {
                        let screen = terminal.screen();
                        let current_virtual_row = screen.scrollback_len() + screen.cursor.y;
                        let line_delta = current_virtual_row.saturating_sub(submit_row);
                        let prompt_line_settled = line_delta >= 1 && screen.cursor.x == submit_x;
                        if prompt_line_settled {
                            prompt_line_settled_in_parse = true;
                            break;
                        }
                    }
                } else {
                    let remaining = MAX_BYTES_PER_TICK.saturating_sub(total_bytes);
                    if remaining == 0 {
                        break;
                    }
                    let take = pty_input_backlog.len().min(4096).min(remaining);
                    if take == 0 {
                        continue;
                    }
                    let mut data = Vec::with_capacity(take);
                    for _ in 0..take {
                        if let Some(byte) = pty_input_backlog.pop_front() {
                            data.push(byte);
                        }
                    }
                    if data.is_empty() {
                        continue;
                    }
                    if !data.iter().all(|b| *b == b'\r' || *b == b'\n') {
                        only_crlf_output = false;
                    }
                    total_bytes += data.len();
                    terminal.process_bytes(&data);
                    let outbound = terminal.take_outbound();
                    if !outbound.is_empty() {
                        let _ = pty.write(&outbound);
                    }
                    got_data = true;
                }
            }
            let new_scrollback = terminal.screen().scrollback_len();
            if got_data {
                scrollback_changed = new_scrollback != old_scrollback;
            }
            let screen = terminal.screen();
            cursor_x = screen.cursor.x;
            cursor_virtual_row = screen.scrollback_len() + screen.cursor.y;
            synchronized_update = terminal.modes.synchronized_update;
            (old_scrollback, new_scrollback)
        };
        self.pty_input_backlog = pty_input_backlog;

        // Adjust selection virtual row indices when scrollback grows/shrinks
        if scrollback_changed {
            let delta = new_scrollback_len as isize - old_scrollback_len as isize;
            if delta > 0 {
                let d = delta as usize;
                if let Some((row, _)) = &mut self.selection_anchor {
                    *row += d;
                }
                if let Some((row, _)) = &mut self.selection_cursor {
                    *row += d;
                }
            } else {
                // Scrollback shrank (clear/reset) — selection is invalid
                self.clear_selection();
            }
        }

        if got_data {
            let now = Instant::now();
            self.last_output_at = Some(now);
            let is_likely_local_echo = self
                .last_input_at
                .map(|last_input| now.duration_since(last_input) <= Self::LOCAL_ECHO_GRACE)
                .unwrap_or(false);
            if is_likely_local_echo {
                self.pending_streaming_ticks = 0;
            } else if !self.output_streaming {
                self.pending_streaming_ticks = self.pending_streaming_ticks.saturating_add(1);
                // Enter streaming only for sustained/heavy PTY output to avoid
                // cursor hide/show flicker on tiny local-echo updates.
                let should_enter_streaming = self.pending_streaming_ticks
                    >= Self::STREAMING_START_TICKS
                    || chunks >= Self::STREAMING_START_CHUNKS
                    || total_bytes >= Self::STREAMING_START_BYTES;
                if should_enter_streaming {
                    self.output_streaming = true;
                    self.pending_streaming_ticks = 0;
                    self.cursor_blink_on = false;
                }
            } else {
                self.pending_streaming_ticks = 0;
                if self.cursor_blink_on {
                    self.cursor_blink_on = false;
                }
            }

            let mut suppress_redraw = false;
            if synchronized_update {
                self.pending_sync_redraw = true;
                suppress_redraw = true;
            } else if self.pending_sync_redraw {
                // Flush once synchronized-update mode ends.
                self.pending_sync_redraw = false;
            }

            if let Some(deadline) = self.enter_coalesce_deadline {
                let (moved_past_prompt_x, prompt_line_settled) =
                    if let (Some(submit_row), Some(submit_x)) =
                        (self.enter_submit_virtual_row, self.enter_submit_cursor_x)
                    {
                        let line_delta = cursor_virtual_row.saturating_sub(submit_row);
                        let prompt_settled = prompt_line_settled_in_parse
                            || (line_delta >= 1 && cursor_x == submit_x);
                        (line_delta > 1 || cursor_x > submit_x, prompt_settled)
                    } else {
                        (false, false)
                    };

                if is_likely_local_echo && prompt_line_settled {
                    // End this cycle immediately once one prompt line settles.
                    self.enter_coalesce_deadline = None;
                    self.enter_coalesce_pending_redraw = false;
                    self.enter_prompt_cursor_x = Some(cursor_x);
                    if self.enter_submit_pending > 0 {
                        self.enter_submit_pending -= 1;
                    }
                    if self.enter_submit_pending > 0 {
                        // Key-repeat submitted more Enter presses: arm next cycle now.
                        self.enter_submit_cursor_x = self.enter_prompt_cursor_x;
                        self.enter_submit_virtual_row = Some(cursor_virtual_row);
                        self.enter_coalesce_deadline = Some(now + Self::ENTER_COALESCE_DELAY);
                    } else {
                        self.enter_submit_cursor_x = None;
                        self.enter_submit_virtual_row = None;
                    }
                } else if is_likely_local_echo {
                    if moved_past_prompt_x {
                        self.enter_coalesce_deadline = None;
                        self.enter_coalesce_pending_redraw = false;
                        self.enter_submit_cursor_x = None;
                        self.enter_submit_virtual_row = None;
                        self.enter_submit_pending = 0;
                    } else {
                        self.enter_coalesce_pending_redraw = true;
                        self.enter_coalesce_deadline = Some(now + Self::ENTER_COALESCE_DELAY);
                        suppress_redraw = true;
                    }
                } else if now < deadline && only_crlf_output {
                    self.enter_coalesce_pending_redraw = true;
                    suppress_redraw = true;
                } else {
                    self.enter_coalesce_deadline = None;
                    self.enter_coalesce_pending_redraw = false;
                    self.enter_submit_cursor_x = None;
                    self.enter_submit_virtual_row = None;
                    self.enter_submit_pending = 0;
                }
            }

            if scrollback_changed {
                if suppress_redraw {
                    self.pending_scroll_clamp = true;
                    self.follow_output = was_at_bottom;
                } else {
                    if was_at_bottom {
                        self.stick_to_bottom(cx);
                    } else {
                        self.clamp_scroll_position(cx);
                    }
                }
            }
            if !suppress_redraw && self.pending_scroll_clamp {
                if self.follow_output {
                    self.stick_to_bottom(cx);
                } else {
                    self.clamp_scroll_position(cx);
                }
                self.pending_scroll_clamp = false;
            }
            if !suppress_redraw {
                self.draw_bg.redraw(cx);
            }
        } else {
            self.pending_streaming_ticks = 0;
            if self.enter_coalesce_deadline.is_some() && self.enter_coalesce_pending_redraw {
                let now = Instant::now();
                if self.enter_waiting_for_local_prompt(now) {
                    self.enter_coalesce_deadline = Some(now + Self::ENTER_COALESCE_DELAY);
                } else {
                    self.enter_coalesce_deadline = None;
                    self.enter_coalesce_pending_redraw = false;
                    self.enter_submit_cursor_x = None;
                    self.enter_submit_virtual_row = None;
                    self.enter_submit_pending = 0;
                    if self.pending_scroll_clamp {
                        if self.follow_output {
                            self.stick_to_bottom(cx);
                        } else {
                            self.clamp_scroll_position(cx);
                        }
                        self.pending_scroll_clamp = false;
                    }
                    self.draw_bg.redraw(cx);
                }
            }
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

    fn pick(&self, abs: Vec2d) -> (usize, usize) {
        let (cell_width, cell_height) = self.cell_metrics();
        let local_x = abs.x - self.unscrolled_rect.pos.x - self.pad_x;
        // Use unscrolled rect + explicit scroll so pick() works correctly
        // even when called after scroll position changes (e.g. auto-scroll)
        let local_y =
            abs.y - self.unscrolled_rect.pos.y - self.pad_y + self.current_scroll_pixels();

        let col = (local_x / cell_width).floor().max(0.0) as usize;
        let row = (local_y / cell_height).floor().max(0.0) as usize;

        let (total_rows, cols) = if let Some(terminal) = &self.terminal {
            (terminal.screen().total_rows(), terminal.screen().cols())
        } else {
            return (0, 0);
        };

        (
            row.min(total_rows.saturating_sub(1)),
            col.min(cols.saturating_sub(1)),
        )
    }

    fn selection_ordered(&self) -> Option<((usize, usize), (usize, usize))> {
        let anchor = self.selection_anchor?;
        let cursor = self.selection_cursor?;
        if anchor == cursor {
            return None;
        }
        if anchor <= cursor {
            Some((anchor, cursor))
        } else {
            Some((cursor, anchor))
        }
    }

    fn selected_text(&self) -> Option<String> {
        let ((start_row, start_col), (end_row, end_col)) = self.selection_ordered()?;
        let terminal = self.terminal.as_ref()?;
        let screen = terminal.screen();
        let cols = screen.cols();
        let mut result = String::new();

        for row in start_row..=end_row {
            let Some(row_slice) = screen.row_slice_virtual(row) else {
                continue;
            };
            let row_start = if row == start_row { start_col } else { 0 };
            let row_end = if row == end_row { end_col } else { cols };
            let row_end = row_end.min(row_slice.len());

            let mut line = String::new();
            for col in row_start..row_end {
                line.push(row_slice[col].codepoint);
            }
            let trimmed = line.trim_end();
            result.push_str(trimmed);
            if row < end_row {
                result.push('\n');
            }
        }
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_cursor = None;
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
        let blank_cell = makepad_terminal_core::Cell::default();

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

        // Predefine draw layer order so interleaved draws land in the right z-order:
        // cell-backgrounds -> selection -> cursor -> text -> decorations.
        self.draw_cell_bg.new_draw_call(cx);
        self.draw_selection.new_draw_call(cx);
        self.draw_cursor.new_draw_call(cx);
        self.draw_text.new_draw_call(cx);
        self.draw_decor.new_draw_call(cx);

        // Draw selection highlight
        let selection = self.selection_ordered();
        if let Some(((sel_start_row, sel_start_col), (sel_end_row, sel_end_col))) = selection {
            let has_focus = cx.has_key_focus(self.scroll_bars.area());
            self.draw_selection.focus = if has_focus { 1.0 } else { 0.0 };
            self.draw_selection.begin();
            for sel_row in sel_start_row..=sel_end_row {
                if sel_row + 1 < top_row {
                    continue;
                }
                if sel_row > top_row + rows + 1 {
                    break;
                }

                let row_start_col = if sel_row == sel_start_row {
                    sel_start_col
                } else {
                    0
                };
                let row_end_col = if sel_row == sel_end_row {
                    sel_end_col
                } else {
                    cols
                };

                if row_start_col == row_end_col {
                    continue;
                }

                let x = origin_x + row_start_col as f64 * cell_width;
                let y = origin_y + sel_row as f64 * cell_height;
                let w = (row_end_col - row_start_col) as f64 * cell_width;

                self.draw_selection.draw(
                    cx,
                    Rect {
                        pos: dvec2(x, y),
                        size: dvec2(w, cell_height),
                    },
                );
            }
            self.draw_selection.end(cx);
        }

        let has_focus = cx.has_key_focus(self.scroll_bars.area());
        self.draw_cursor.focus = if has_focus { 1.0 } else { 0.0 };

        // Cursor
        if terminal.modes.cursor_visible && !self.output_streaming {
            let cursor = &screen.cursor;
            let cursor_virtual_y = screen.scrollback_len() + cursor.y;
            let cursor_content_y = cursor_virtual_y as f64 * cell_height;
            if !(cursor_content_y + cell_height < scroll_y
                || cursor_content_y > scroll_y + self.viewport_rect.size.y)
            {
                let cx_x = origin_x + cursor.x as f64 * cell_width;
                let cx_y = origin_y + cursor_content_y + self.cursor_y_offset;

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

        // Draw cells — interleaved bg/text/decor appends to predefined layers.
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
                    self.draw_cell_bg.color = vec4(
                        bg_r as f32 / 255.0,
                        bg_g as f32 / 255.0,
                        bg_b as f32 / 255.0,
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

                let blink_hidden = flags.has(StyleFlags::BLINK) && !self.cursor_blink_on;
                let ch = cell.codepoint;
                if ch != ' ' && ch != '\0' && !blink_hidden && !flags.has(StyleFlags::INVISIBLE) {
                    let mut s = [0u8; 4];
                    let text = ch.encode_utf8(&mut s);
                    self.draw_text.color = vec4(
                        fg_r as f32 / 255.0,
                        fg_g as f32 / 255.0,
                        fg_b as f32 / 255.0,
                        1.0,
                    );
                    self.draw_text.draw_abs(
                        cx,
                        dvec2(x, y + self.cell_offset_y + self.text_y_offset),
                        text,
                    );
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
        self.unscrolled_rect = cx.turtle().rect_unscrolled();
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
        let enter_coalescing = self.enter_coalesce_deadline.is_some();

        if visible {
            self.ensure_pty(cx);

            if !enter_coalescing && self.scroll_bars.handle_event(cx, event, scope).len() > 0 {
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
                        if !self.enter_coalesce_deadline.is_some() {
                            if self.follow_output {
                                self.stick_to_bottom(cx);
                            } else {
                                self.clamp_scroll_position(cx);
                            }
                            self.pending_scroll_clamp = false;
                        }
                    }
                    self.poll_pty_spawn(cx);
                    self.poll_pty_output(cx);
                    self.update_output_streaming_state(cx);
                    self.update_enter_coalesce_state(cx);
                }
                if self.cursor_blink_timer.is_timer(te).is_some() {
                    if !visible {
                        return;
                    }
                    if self.enter_coalesce_deadline.is_some() {
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

        // Auto-scroll during drag selection
        if self.selecting {
            if self.select_scroll_next_frame.is_event(event).is_some() {
                self.select_scroll_next_frame = cx.new_next_frame();
                if let Some(abs) = self.last_finger_abs {
                    let vp_top = self.viewport_rect.pos.y;
                    let vp_bottom = vp_top + self.viewport_rect.size.y;
                    let (_, cell_height) = self.cell_metrics();
                    let scroll_speed = cell_height * 2.0;

                    if abs.y < vp_top {
                        let delta = (vp_top - abs.y).min(scroll_speed);
                        let new_y = (self.current_scroll_pixels() - delta).max(0.0);
                        let _ = self
                            .scroll_bars
                            .set_scroll_pos_no_clip(cx, dvec2(0.0, new_y));
                    } else if abs.y > vp_bottom {
                        let delta = (abs.y - vp_bottom).min(scroll_speed);
                        let new_y =
                            (self.current_scroll_pixels() + delta).min(self.max_scroll_pixels());
                        let _ = self
                            .scroll_bars
                            .set_scroll_pos_no_clip(cx, dvec2(0.0, new_y));
                    }

                    self.selection_cursor = Some(self.pick(abs));
                    self.draw_bg.redraw(cx);
                }
            }
        }

        if !visible {
            return;
        }

        match event.hits(cx, self.scroll_bars.area()) {
            Hit::FingerDown(FingerDownEvent { abs, .. }) => {
                cx.set_key_focus(self.scroll_bars.area());
                self.cursor_blink_on = true;
                let pos = self.pick(abs);
                self.selection_anchor = Some(pos);
                self.selection_cursor = Some(pos);
                self.selecting = true;
                self.last_finger_abs = Some(abs);
                self.select_scroll_next_frame = cx.new_next_frame();
                self.draw_bg.redraw(cx);
            }
            Hit::FingerMove(FingerMoveEvent { abs, .. }) => {
                cx.set_cursor(MouseCursor::Text);
                self.selection_cursor = Some(self.pick(abs));
                self.last_finger_abs = Some(abs);
                self.draw_bg.redraw(cx);
            }
            Hit::FingerUp(_) => {
                self.selecting = false;
                self.last_finger_abs = None;
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Text);
            }
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => {
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(e) => {
                let is_enter = matches!(e.key_code, KeyCode::ReturnKey | KeyCode::NumpadEnter);
                if is_enter {
                    self.note_local_input_preserve_enter_coalesce(cx);
                    self.note_return_submit();
                } else {
                    self.note_local_input(cx);
                }
                match e.key_code {
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
                let is_newline_text = matches!(e.input.as_str(), "\n" | "\r" | "\r\n");
                if !e.was_paste && is_newline_text {
                    // Return is sent via KeyDown; skip duplicate TextInput newline.
                } else {
                    self.note_local_input(cx);
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
            }
            Hit::TextCopy(ce) => {
                if let Some(text) = self.selected_text() {
                    *ce.response.borrow_mut() = Some(text);
                }
            }
            _ => {}
        }
    }
}

fn map_keycode(kc: KeyCode) -> TermKeyCode {
    use makepad_terminal_core::TermKeyCode as TK;
    match kc {
        KeyCode::ReturnKey => TK::Return,
        KeyCode::NumpadEnter => TK::Return,
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
