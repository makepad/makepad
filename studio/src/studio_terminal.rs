use crate::makepad_code_editor::draw_selection::DrawSelection;
use crate::makepad_widgets::text::geom::Point;
use crate::makepad_widgets::text::rasterizer::RasterizedGlyph;
use crate::makepad_widgets::*;
use makepad_terminal_core::{Color, CursorShape, Pty, StyleFlags, TermKeyCode, Terminal};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::OnceLock;
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
            draw_call_group: @text
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

static TERMINAL_START_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_terminal_start_dir(path: PathBuf) {
    let _ = TERMINAL_START_DIR.set(path);
}

fn terminal_start_dir() -> Option<PathBuf> {
    TERMINAL_START_DIR.get().cloned()
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

#[derive(Clone, Copy)]
struct CachedTerminalGlyph {
    rasterized: RasterizedGlyph,
    font_size_in_lpxs: f32,
    x_offset_in_lpxs: f32,
    baseline_offset_in_lpxs: f32,
}

struct EnterCoalesce {
    /// Cursor X at the moment Enter was pressed. We scan the raw backlog
    /// for a newline followed by >= this many printable chars.
    target_x: usize,
    /// Soft deadline — after this we may flush unless that would reveal a
    /// just-scrolled blank line.
    deadline: Instant,
    /// Hard deadline — always flush by this time.
    hard_deadline: Instant,
}

/// While the terminal cursor hasn't settled to the prompt position after
/// Enter, we keep transition state so redraw/scroll clamping can defer
/// until settle is complete.
struct CursorHold {
    /// Original virtual row at Enter; used for release checks even when
    /// viewport/scrollback state changes while coalescing.
    release_virtual_row: usize,
    /// We expect the cursor to reach at least this X on a row AFTER `virtual_row`.
    target_x: usize,
    /// Safety deadline — don't hold forever.
    deadline: Instant,
}

struct EnterPromptScan {
    settled: bool,
    saw_newline: bool,
    saw_visible_after_newline: bool,
}

#[derive(Clone, Debug, Default)]
pub enum StudioTerminalAction {
    SetTabTitle(String),
    #[default]
    None,
}

#[derive(Clone, Copy, Default)]
enum TitleMarkerParseState {
    #[default]
    Ground,
    OpenBrace,
    InMarker,
    MarkerCloseBrace,
}

#[derive(Clone, Copy)]
enum EnterScanState {
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
    String,
    StringEsc,
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
    /// When set, we're coalescing: buffer PTY data byte-by-byte into the
    /// terminal, but don't redraw until cursor.x >= this value on a new
    /// line, or the deadline expires.
    #[rust]
    enter_coalesce: Option<EnterCoalesce>,
    #[rust]
    cursor_hold: Option<CursorHold>,
    #[rust]
    pty_input_backlog: VecDeque<u8>,
    #[rust]
    last_output_at: Option<Instant>,
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
    #[rust]
    title_marker_parse_state: TitleMarkerParseState,
    #[rust]
    title_marker_parse_buf: Vec<u8>,
}

impl ScriptHook for StudioTerminal {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.with_cx_mut(|cx| {
            self.ensure_pty(cx);
        });
    }
}

impl StudioTerminal {
    fn debug_log_draw_list(&self, cx: &mut Cx2d) {
        if std::env::var_os("MAKEPAD_TERM_DRAWLIST_DEBUG").is_none() {
            return;
        }
        static LAST_REDRAW_ID: AtomicU64 = AtomicU64::new(0);
        static LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
        let redraw_id = cx.redraw_id;
        let prev = LAST_REDRAW_ID.swap(redraw_id, Ordering::Relaxed);
        if prev == redraw_id {
            return;
        }
        if LOG_COUNT.fetch_add(1, Ordering::Relaxed) > 120 {
            return;
        }

        let Some(draw_list_id) = cx.draw_list_stack.last().copied() else {
            return;
        };
        let draw_list = &cx.draw_lists[draw_list_id];
        log!(
            "term_drawlist redraw={} list={:?} items={}",
            redraw_id,
            draw_list_id,
            draw_list.draw_items.len()
        );
        for i in 0..draw_list.draw_items.len() {
            let item = &draw_list.draw_items[i];
            if let Some(draw_call) = item.draw_call() {
                let shader = &cx.draw_shaders[draw_call.draw_shader_id.index];
                let instance_floats = item.instances.as_ref().map_or(0, |v| v.len());
                log!(
                    "  [{}] shader_idx={} shader_dbg={} append_group={} draw_call_group={} inst_floats={}",
                    i,
                    draw_call.draw_shader_id.index,
                    shader.debug_id,
                    draw_call.append_group_id,
                    draw_call.options.draw_call_group.0,
                    instance_floats
                );
            } else if let Some(sub_list_id) = item.sub_list() {
                log!("  [{}] sub_list={:?}", i, sub_list_id);
            } else {
                log!("  [{}] empty", i);
            }
        }
    }

    const OUTPUT_QUIET_DELAY: Duration = Duration::from_millis(120);
    const STREAMING_START_TICKS: u8 = 2;
    const STREAMING_START_BYTES: usize = 1024;
    const TITLE_MARKER_MAX_BYTES: usize = 160;
    const TITLE_MAX_CHARS: usize = 48;
    /// Maximum time to wait for prompt to settle after Enter before flushing redraw.
    const ENTER_COALESCE_TIMEOUT: Duration = Duration::from_millis(30);
    /// Maximum time to hold the cursor at the saved position after Enter.
    /// Slightly longer than coalesce timeout to cover the frame(s) where
    /// partial data is being processed.
    const CURSOR_HOLD_TIMEOUT: Duration = Duration::from_millis(150);

    fn is_image_path(path: &str) -> bool {
        let Some(ext) = Path::new(path).extension().and_then(|ext| ext.to_str()) else {
            return false;
        };
        let ext = ext.to_ascii_lowercase();
        matches!(
            ext.as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "bmp"
                | "tif"
                | "tiff"
                | "heic"
                | "heif"
                | "avif"
        )
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
                if let (Some(hi), Some(lo)) =
                    (Self::hex_nibble(bytes[i + 1]), Self::hex_nibble(bytes[i + 2]))
                {
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

    fn dropped_image_paths(items: &[DragItem]) -> Option<Vec<String>> {
        let mut paths = Vec::new();
        for item in items {
            match item {
                DragItem::FilePath { path, internal_id } => {
                    if internal_id.is_some() {
                        return None;
                    }
                    let path = Self::decode_percent_escapes(path);
                    if !Self::is_image_path(&path) {
                        return None;
                    }
                    paths.push(path);
                }
                _ => return None,
            }
        }
        if paths.is_empty() {
            None
        } else {
            Some(paths)
        }
    }

    fn send_dropped_image_paths(&mut self, cx: &mut Cx, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        self.ensure_pty(cx);
        let mut payload = String::new();
        for (index, path) in paths.iter().enumerate() {
            payload.push_str(&Self::shell_quote_path(path));
            if index + 1 < paths.len() {
                payload.push(' ');
            }
        }
        payload.push(' ');
        self.note_local_input(cx);
        self.send_text_to_pty(&payload, &KeyModifiers::default());
    }

    pub fn insert_dropped_image_paths(&mut self, cx: &mut Cx, paths: &[String]) {
        self.send_dropped_image_paths(cx, paths);
    }

    fn parse_tab_title_marker(marker: &str) -> Option<String> {
        let marker = marker.trim();
        if marker.is_empty() {
            return None;
        }

        let title = if marker.len() >= 5 {
            let (prefix, rest) = marker.split_at(5);
            if prefix.eq_ignore_ascii_case("title") {
                let mut rest = rest.trim_start();
                if let Some(stripped) = rest.strip_prefix(':').or_else(|| rest.strip_prefix('='))
                {
                    rest = stripped.trim_start();
                }
                if rest.is_empty() {
                    marker
                } else {
                    rest
                }
            } else {
                marker
            }
        } else {
            marker
        };

        let out = title
            .chars()
            .filter(|ch| !ch.is_control())
            .take(Self::TITLE_MAX_CHARS)
            .collect::<String>();

        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    fn emit_title_marker_literal(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"{{");
        out.extend_from_slice(&self.title_marker_parse_buf);
        if matches!(
            self.title_marker_parse_state,
            TitleMarkerParseState::MarkerCloseBrace
        ) {
            out.push(b'}');
        }
    }

    fn take_incomplete_title_marker_literal(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        match self.title_marker_parse_state {
            TitleMarkerParseState::Ground => {}
            TitleMarkerParseState::OpenBrace => {
                out.push(b'{');
            }
            TitleMarkerParseState::InMarker | TitleMarkerParseState::MarkerCloseBrace => {
                self.emit_title_marker_literal(&mut out);
            }
        }
        self.title_marker_parse_buf.clear();
        self.title_marker_parse_state = TitleMarkerParseState::Ground;
        out
    }

    fn flush_stale_incomplete_title_marker(&mut self, cx: &mut Cx) {
        if matches!(self.title_marker_parse_state, TitleMarkerParseState::Ground) {
            return;
        }
        let Some(last_output) = self.last_output_at else {
            return;
        };
        if last_output.elapsed() < Self::OUTPUT_QUIET_DELAY {
            return;
        }
        let literal = self.take_incomplete_title_marker_literal();
        if literal.is_empty() {
            return;
        }
        if let Some(terminal) = &mut self.terminal {
            terminal.process_bytes(&literal);
            let outbound = terminal.take_outbound();
            if !outbound.is_empty() {
                if let Some(pty) = &self.pty {
                    let _ = pty.write(&outbound);
                }
            }
            self.draw_bg.redraw(cx);
        }
    }

    fn parse_title_markers(&mut self, cx: &mut Cx, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());

        for &byte in input {
            match self.title_marker_parse_state {
                TitleMarkerParseState::Ground => {
                    if byte == b'{' {
                        self.title_marker_parse_state = TitleMarkerParseState::OpenBrace;
                    } else {
                        out.push(byte);
                    }
                }
                TitleMarkerParseState::OpenBrace => {
                    if byte == b'{' {
                        self.title_marker_parse_state = TitleMarkerParseState::InMarker;
                        self.title_marker_parse_buf.clear();
                    } else {
                        out.push(b'{');
                        out.push(byte);
                        self.title_marker_parse_state = TitleMarkerParseState::Ground;
                    }
                }
                TitleMarkerParseState::InMarker => {
                    if byte == b'}' {
                        self.title_marker_parse_state = TitleMarkerParseState::MarkerCloseBrace;
                    } else if byte == b'\n' || byte == b'\r' {
                        self.emit_title_marker_literal(&mut out);
                        out.push(byte);
                        self.title_marker_parse_buf.clear();
                        self.title_marker_parse_state = TitleMarkerParseState::Ground;
                    } else if self.title_marker_parse_buf.len() >= Self::TITLE_MARKER_MAX_BYTES {
                        self.emit_title_marker_literal(&mut out);
                        out.push(byte);
                        self.title_marker_parse_buf.clear();
                        self.title_marker_parse_state = TitleMarkerParseState::Ground;
                    } else {
                        self.title_marker_parse_buf.push(byte);
                    }
                }
                TitleMarkerParseState::MarkerCloseBrace => {
                    if byte == b'}' {
                        let marker = String::from_utf8_lossy(&self.title_marker_parse_buf);
                        if let Some(title) = Self::parse_tab_title_marker(marker.as_ref()) {
                            cx.widget_action(self.widget_uid(), StudioTerminalAction::SetTabTitle(title));
                        } else {
                            self.emit_title_marker_literal(&mut out);
                            out.push(b'}');
                        }
                        self.title_marker_parse_buf.clear();
                        self.title_marker_parse_state = TitleMarkerParseState::Ground;
                    } else if byte == b'\n' || byte == b'\r' {
                        self.emit_title_marker_literal(&mut out);
                        out.push(byte);
                        self.title_marker_parse_buf.clear();
                        self.title_marker_parse_state = TitleMarkerParseState::Ground;
                    } else if self.title_marker_parse_buf.len() >= Self::TITLE_MARKER_MAX_BYTES {
                        self.emit_title_marker_literal(&mut out);
                        out.push(byte);
                        self.title_marker_parse_buf.clear();
                        self.title_marker_parse_state = TitleMarkerParseState::Ground;
                    } else {
                        self.title_marker_parse_buf.push(b'}');
                        self.title_marker_parse_buf.push(byte);
                        self.title_marker_parse_state = TitleMarkerParseState::InMarker;
                    }
                }
            }
        }

        out
    }

    fn handle_image_file_drop(&mut self, cx: &mut Cx, event: &Event) -> bool {
        let drop_rect = self.scroll_bars.area().clipped_rect(cx);
        match event {
            Event::Drag(drag_event) => {
                if *drag_event.handled.lock().unwrap() || !drop_rect.contains(drag_event.abs) {
                    return false;
                }
                if Self::dropped_image_paths(drag_event.items.as_ref()).is_none() {
                    return false;
                }
                *drag_event.response.lock().unwrap() = DragResponse::Copy;
                *drag_event.handled.lock().unwrap() = true;
                true
            }
            Event::Drop(drop_event) => {
                if *drop_event.handled.lock().unwrap() || !drop_rect.contains(drop_event.abs) {
                    return false;
                }
                let Some(paths) = Self::dropped_image_paths(drop_event.items.as_ref()) else {
                    return false;
                };
                *drop_event.handled.lock().unwrap() = true;
                self.send_dropped_image_paths(cx, &paths);
                true
            }
            _ => false,
        }
    }

    fn invalidate_glyph_cache_if_needed(
        glyph_cache: &mut HashMap<char, CachedTerminalGlyph>,
        glyph_cache_font_size: &mut f32,
        glyph_cache_font_scale: &mut f32,
        glyph_cache_dpi_factor: &mut f64,
        draw_text: &DrawText,
        dpi_factor: f64,
    ) {
        let font_size = draw_text.text_style.font_size;
        let font_scale = draw_text.font_scale;
        if glyph_cache_font_size.to_bits() == font_size.to_bits()
            && glyph_cache_font_scale.to_bits() == font_scale.to_bits()
            && glyph_cache_dpi_factor.to_bits() == dpi_factor.to_bits()
        {
            return;
        }
        glyph_cache.clear();
        *glyph_cache_font_size = font_size;
        *glyph_cache_font_scale = font_scale;
        *glyph_cache_dpi_factor = dpi_factor;
    }

    fn cached_terminal_glyph(
        draw_text: &mut DrawText,
        glyph_cache: &mut HashMap<char, CachedTerminalGlyph>,
        cx: &mut Cx2d,
        ch: char,
    ) -> Option<CachedTerminalGlyph> {
        if let Some(cached) = glyph_cache.get(&ch) {
            return Some(*cached);
        }

        let mut utf8 = [0u8; 4];
        let text = ch.encode_utf8(&mut utf8);
        let run = draw_text.prepare_single_line_run(cx, text)?;
        let glyph = run.glyphs.first()?;
        let cached = CachedTerminalGlyph {
            rasterized: glyph.rasterized,
            font_size_in_lpxs: glyph.font_size_in_lpxs,
            x_offset_in_lpxs: glyph.pen_x_in_lpxs + glyph.offset_x_in_lpxs,
            baseline_offset_in_lpxs: run.ascender_in_lpxs,
        };
        glyph_cache.insert(ch, cached);
        Some(cached)
    }

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

    fn scan_enter_prompt_settle(&self, target_x: usize) -> EnterPromptScan {
        let mut chars_after_newline = 0usize;
        let mut saw_newline = false;
        let mut saw_visible_after_newline = false;
        let mut state = EnterScanState::Ground;

        for &byte in self.pty_input_backlog.iter() {
            match state {
                EnterScanState::Ground => match byte {
                    b'\n' | b'\r' => {
                        saw_newline = true;
                        chars_after_newline = 0;
                    }
                    0x1b => {
                        state = EnterScanState::Esc;
                    }
                    _ => {
                        if saw_newline && byte >= 0x20 && byte != 0x7f {
                            saw_visible_after_newline = true;
                            chars_after_newline += 1;
                            if chars_after_newline >= target_x {
                                return EnterPromptScan {
                                    settled: true,
                                    saw_newline,
                                    saw_visible_after_newline,
                                };
                            }
                        }
                    }
                },
                EnterScanState::Esc => {
                    state = match byte {
                        b'[' => EnterScanState::Csi,
                        b']' => EnterScanState::Osc,
                        b'P' | b'X' | b'^' | b'_' => EnterScanState::String,
                        _ => EnterScanState::Ground,
                    };
                }
                EnterScanState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        state = EnterScanState::Ground;
                    }
                }
                EnterScanState::Osc => {
                    if byte == 0x07 {
                        state = EnterScanState::Ground;
                    } else if byte == 0x1b {
                        state = EnterScanState::OscEsc;
                    }
                }
                EnterScanState::OscEsc => {
                    if byte == b'\\' {
                        state = EnterScanState::Ground;
                    } else {
                        state = EnterScanState::Osc;
                    }
                }
                EnterScanState::String => {
                    if byte == 0x1b {
                        state = EnterScanState::StringEsc;
                    }
                }
                EnterScanState::StringEsc => {
                    if byte == b'\\' {
                        state = EnterScanState::Ground;
                    } else {
                        state = EnterScanState::String;
                    }
                }
            }
        }

        EnterPromptScan {
            settled: false,
            saw_newline,
            saw_visible_after_newline,
        }
    }

    fn update_enter_coalesce_state(&mut self, cx: &mut Cx) {
        if let Some(ref coal) = self.enter_coalesce {
            if Instant::now() >= coal.hard_deadline {
                self.enter_coalesce = None;
            }
        }
        self.update_cursor_hold_state(cx);
    }

    fn update_cursor_hold_state(&mut self, cx: &mut Cx) {
        let Some(hold) = self.cursor_hold.as_ref() else {
            return;
        };

        let mut release = Instant::now() >= hold.deadline;
        if !release {
            if let Some(terminal) = &self.terminal {
                let screen = terminal.screen();
                let cur_virtual_row = screen.scrollback_len() + screen.cursor.y;
                if cur_virtual_row > hold.release_virtual_row && screen.cursor.x >= hold.target_x {
                    release = true;
                }
            }
        }

        if release {
            self.cursor_hold = None;
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

    /// Called when the user presses Enter.
    fn note_enter_pressed(&mut self, cx: &mut Cx) {
        let mut redraw = false;
        if self.output_streaming {
            self.output_streaming = false;
            redraw = true;
        }
        if !self.cursor_blink_on {
            self.cursor_blink_on = true;
            redraw = true;
        }

        if let Some(terminal) = &self.terminal {
            let screen = terminal.screen();
            // For coalescing we just need to see *some* content on the new
            // line (the prompt). Using 1 avoids the problem where a long
            // command like "ls -al" sets target_x too high for the short
            // prompt to reach.
            let target_x = 1;
            let now = Instant::now();
            self.enter_coalesce = Some(EnterCoalesce {
                target_x,
                deadline: now + Self::ENTER_COALESCE_TIMEOUT,
                hard_deadline: now + Self::CURSOR_HOLD_TIMEOUT,
            });
            // Only set cursor hold if there isn't one already (rapid Enter
            // should keep the original saved position, not overwrite with
            // an intermediate cursor position).
            if self.cursor_hold.is_none() {
                let virtual_row = screen.scrollback_len() + screen.cursor.y;
                self.cursor_hold = Some(CursorHold {
                    release_virtual_row: virtual_row,
                    target_x,
                    deadline: Instant::now() + Self::CURSOR_HOLD_TIMEOUT,
                });
            } else {
                // Extend the existing hold's deadline
                if let Some(ref mut hold) = self.cursor_hold {
                    hold.deadline = Instant::now() + Self::CURSOR_HOLD_TIMEOUT;
                }
            }
        }

        if redraw {
            self.draw_bg.redraw(cx);
        }
    }

    fn note_local_input(&mut self, cx: &mut Cx) {
        self.pending_streaming_ticks = 0;
        self.enter_coalesce = None;
        self.cursor_hold = None;
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

        let start_dir = terminal_start_dir();
        if std::thread::Builder::new()
            .name("studio-pty-spawn".to_string())
            .spawn(move || {
                let child_env = [
                    ("COLORTERM", "truecolor"),
                    ("TERM_PROGRAM", "makepad-studio"),
                    ("TERM_PROGRAM_VERSION", "0.1"),
                ];
                let _ = tx.send(Pty::spawn(80, 24, None, &child_env, start_dir.as_deref()));
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

        if self.pty.is_none() {
            return;
        }
        const MAX_BYTES_PER_TICK: usize = 1 << 20;

        // Read all available PTY data into the backlog
        let mut fresh_bytes = 0usize;
        loop {
            let Some(data) = self.pty.as_ref().and_then(|pty| pty.try_read()) else {
                break;
            };
            fresh_bytes += data.len();
            self.pty_input_backlog.extend(data);
            if fresh_bytes >= MAX_BYTES_PER_TICK {
                break;
            }
        }
        if fresh_bytes > 0 {
            self.last_output_at = Some(Instant::now());
        }

        if self.pty_input_backlog.is_empty() {
            self.pending_streaming_ticks = 0;
            self.update_cursor_hold_state(cx);
            self.flush_stale_incomplete_title_marker(cx);
            return;
        }

        // During coalescing: DON'T process bytes into the terminal.
        // Just scan the raw backlog to detect if the next prompt has
        // arrived (a newline followed by >= target_x visible chars).
        // This keeps the terminal screen buffer frozen so any redraws
        // from parent widgets show the clean pre-Enter frame.
        if let Some(ref coal) = self.enter_coalesce {
            let scan = self.scan_enter_prompt_settle(coal.target_x);
            if !scan.settled {
                let now = Instant::now();
                if now < coal.deadline {
                    // Not settled yet — keep buffering, don't process.
                    return;
                }
                if scan.saw_newline && !scan.saw_visible_after_newline && now < coal.hard_deadline {
                    // We've already advanced to a new line, but there is still
                    // no visible content for it. Keep coalescing so we don't
                    // briefly reveal an empty scrolled line.
                    return;
                }
            }
            self.enter_coalesce = None;
        }

        // Process all backlog bytes through the terminal emulator.
        let Some(old_sb) = self.terminal.as_ref().map(|terminal| terminal.screen().scrollback_len())
        else {
            return;
        };

        let mut total = 0usize;
        let mut filtered_chunks = Vec::new();
        while !self.pty_input_backlog.is_empty() {
            let take = self.pty_input_backlog.len().min(4096);
            let mut data = Vec::with_capacity(take);
            for _ in 0..take {
                if let Some(b) = self.pty_input_backlog.pop_front() {
                    data.push(b);
                }
            }
            let filtered = self.parse_title_markers(cx, &data);
            total += filtered.len();
            if !filtered.is_empty() {
                filtered_chunks.push(filtered);
            }
        }

        let (new_sb, synchronized_update) = {
            let Some(terminal) = &mut self.terminal else {
                return;
            };
            for chunk in filtered_chunks {
                terminal.process_bytes(&chunk);
                let outbound = terminal.take_outbound();
                if !outbound.is_empty() {
                    if let Some(pty) = &self.pty {
                        let _ = pty.write(&outbound);
                    }
                }
            }
            (terminal.screen().scrollback_len(), terminal.modes.synchronized_update)
        };

        let total_bytes = total;
        let old_scrollback = old_sb;
        let new_scrollback = new_sb;

        if total_bytes == 0 {
            return;
        }

        // Adjust selection and cursor hold when scrollback changes
        let scrollback_changed = new_scrollback != old_scrollback;
        if scrollback_changed {
            let delta = new_scrollback as isize - old_scrollback as isize;
            if delta > 0 {
                let d = delta as usize;
                if let Some((row, _)) = &mut self.selection_anchor {
                    *row += d;
                }
                if let Some((row, _)) = &mut self.selection_cursor {
                    *row += d;
                }
            } else {
                self.clear_selection();
            }
        }

        self.update_cursor_hold_state(cx);

        // Any real scrollback movement means active output is still in flight.
        // Enter streaming mode immediately so the cursor doesn't blink while
        // rows are shifting. Keep the held cursor visible during post-Enter
        // settling, so skip this while cursor_hold is active.
        if scrollback_changed
            && !self.output_streaming
            && self.cursor_hold.is_none()
            && self.enter_coalesce.is_none()
        {
            self.output_streaming = true;
            self.pending_streaming_ticks = 0;
            self.cursor_blink_on = false;
        }

        // Streaming detection
        if !self.output_streaming {
            self.pending_streaming_ticks = self.pending_streaming_ticks.saturating_add(1);
            if self.pending_streaming_ticks >= Self::STREAMING_START_TICKS
                || total_bytes >= Self::STREAMING_START_BYTES
            {
                if self.cursor_hold.is_none() && self.enter_coalesce.is_none() {
                    self.output_streaming = true;
                    self.pending_streaming_ticks = 0;
                    self.cursor_blink_on = false;
                } else {
                    self.pending_streaming_ticks = 0;
                }
            }
        } else if self.cursor_blink_on
            && self.cursor_hold.is_none()
            && self.enter_coalesce.is_none()
        {
            self.cursor_blink_on = false;
        }

        // Synchronized update mode (DEC 2026)
        if synchronized_update {
            self.pending_sync_redraw = true;
            self.pending_scroll_clamp = true;
            self.follow_output = was_at_bottom;
            return;
        } else if self.pending_sync_redraw {
            self.pending_sync_redraw = false;
        }

        // Scroll and redraw
        if scrollback_changed {
            // Keep viewport locked while cursor_hold is active so we don't
            // briefly reveal an empty just-scrolled line.
            if self.cursor_hold.is_some() && was_at_bottom {
                self.pending_scroll_clamp = true;
                self.follow_output = true;
            } else if was_at_bottom {
                self.stick_to_bottom(cx);
            } else {
                self.clamp_scroll_position(cx);
            }
        }
        if self.pending_scroll_clamp && self.cursor_hold.is_none() {
            if was_at_bottom {
                self.follow_output = true;
            }
            if self.follow_output {
                self.stick_to_bottom(cx);
            } else {
                self.clamp_scroll_position(cx);
            }
            self.pending_scroll_clamp = false;
        }
        self.draw_bg.redraw(cx);
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

    fn word_kind(ch: char) -> Option<bool> {
        if ch == '\0' || ch.is_whitespace() {
            None
        } else {
            Some(ch.is_alphanumeric() || ch == '_')
        }
    }

    fn word_range_at(&self, row: usize, col: usize) -> Option<(usize, usize)> {
        let terminal = self.terminal.as_ref()?;
        let row_slice = terminal.screen().row_slice_virtual(row)?;
        if row_slice.is_empty() {
            return None;
        }

        let col = col.min(row_slice.len().saturating_sub(1));
        let kind = Self::word_kind(row_slice[col].codepoint)?;

        let mut start = col;
        while start > 0 {
            if Self::word_kind(row_slice[start - 1].codepoint) != Some(kind) {
                break;
            }
            start -= 1;
        }

        let mut end = col + 1;
        while end < row_slice.len() {
            if Self::word_kind(row_slice[end].codepoint) != Some(kind) {
                break;
            }
            end += 1;
        }

        Some((start, end))
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

        let (cell_width, cell_height) = self.cell_metrics();
        let origin_x = self.viewport_rect.pos.x + self.pad_x;
        let origin_y = self.viewport_rect.pos.y + self.pad_y;
        let scroll_y = self.current_scroll_pixels();

        let max_scroll_rows = Self::max_scroll_rows(screen);
        let top_row = self.current_scroll_rows().min(max_scroll_rows);
        let total_virtual_rows = screen.total_rows();
        let viewport_rows = (self.viewport_rect.size.y / cell_height).ceil().max(0.0) as usize;
        // Draw one extra row beyond the visible viewport so a partially visible
        // last line is not clipped out early while scrolling.
        let draw_end_row_exclusive = top_row
            .saturating_add(viewport_rows)
            .saturating_add(1)
            .min(total_virtual_rows);
        let last_draw_row = draw_end_row_exclusive.saturating_sub(1);

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
        Self::invalidate_glyph_cache_if_needed(
            &mut self.glyph_cache,
            &mut self.glyph_cache_font_size,
            &mut self.glyph_cache_font_scale,
            &mut self.glyph_cache_dpi_factor,
            &self.draw_text,
            cx.current_dpi_factor(),
        );

        // Draw selection highlight
        let selection = self.selection_ordered();
        if let Some(((sel_start_row, sel_start_col), (sel_end_row, sel_end_col))) = selection {
            let has_focus = cx.has_key_focus(self.scroll_bars.area());
            self.draw_selection.focus = if has_focus { 1.0 } else { 0.0 };
            self.draw_selection.begin();
            for sel_row in sel_start_row..=sel_end_row {
                if sel_row < top_row {
                    continue;
                }
                if sel_row > last_draw_row {
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

        // Cursor: hide it while Enter settle/coalesce is active.
        // This avoids showing a stale cursor on the previous line and then
        // jumping it once the new prompt position is committed.
        if terminal.modes.cursor_visible
            && !self.output_streaming
            && self.cursor_hold.is_none()
            && self.enter_coalesce.is_none()
        {
            let cursor = &screen.cursor;
            let cursor_content_y = (screen.scrollback_len() + cursor.y) as f64 * cell_height;
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
        self.draw_text.begin_many_instances(cx);
        for virtual_row in top_row..draw_end_row_exclusive {
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
                    let color = vec4(
                        fg_r as f32 / 255.0,
                        fg_g as f32 / 255.0,
                        fg_b as f32 / 255.0,
                        1.0,
                    );
                    if let Some(glyph) = Self::cached_terminal_glyph(
                        &mut self.draw_text,
                        &mut self.glyph_cache,
                        cx,
                        ch,
                    ) {
                        let baseline_y = y
                            + self.cell_offset_y
                            + self.text_y_offset
                            + glyph.baseline_offset_in_lpxs as f64;
                        let origin_x = x + glyph.x_offset_in_lpxs as f64;
                        self.draw_text.draw_rasterized_glyph_abs(
                            cx,
                            Point::new(origin_x as f32, baseline_y as f32),
                            glyph.font_size_in_lpxs,
                            glyph.rasterized,
                            color,
                        );
                        if flags.has(StyleFlags::BOLD) {
                            self.draw_text.draw_rasterized_glyph_abs(
                                cx,
                                Point::new((origin_x + 0.6) as f32, baseline_y as f32),
                                glyph.font_size_in_lpxs,
                                glyph.rasterized,
                                color,
                            );
                        }
                    } else {
                        let mut s = [0u8; 4];
                        let text = ch.encode_utf8(&mut s);
                        self.draw_text.color = color;
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
        self.draw_text.end_many_instances(cx);
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
        self.debug_log_draw_list(cx);
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

            if self.handle_image_file_drop(cx, event) {
                return;
            }

            if self.scroll_bars.handle_event(cx, event, scope).len() > 0 {
                if self.enter_coalesce.is_none() {
                    if let Some(terminal) = &self.terminal {
                        self.follow_output = self.is_scrolled_to_bottom(terminal.screen());
                    }
                    self.draw_bg.redraw(cx);
                }
            }
        }

        match event {
            Event::Timer(te) => {
                if self.poll_timer.is_timer(te).is_some() {
                    if !visible {
                        return;
                    }
                    self.poll_pty_spawn(cx);
                    self.update_enter_coalesce_state(cx);
                    self.poll_pty_output(cx);
                    self.update_output_streaming_state(cx);
                }
                if self.cursor_blink_timer.is_timer(te).is_some() {
                    if !visible {
                        return;
                    }
                    if self.enter_coalesce.is_some() || self.cursor_hold.is_some() {
                        if !self.cursor_blink_on {
                            self.cursor_blink_on = true;
                            self.draw_bg.redraw(cx);
                        }
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
                    // Pointer coordinates are absolute, so compare against the
                    // unscrolled viewport bounds (also absolute).
                    let vp_top = self.unscrolled_rect.pos.y;
                    let vp_bottom = vp_top + self.unscrolled_rect.size.y;
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
            Hit::FingerDown(FingerDownEvent { abs, tap_count, .. }) => {
                cx.set_key_focus(self.scroll_bars.area());
                self.cursor_blink_on = true;
                let pos = self.pick(abs);
                if tap_count == 2 {
                    if let Some((start_col, end_col)) = self.word_range_at(pos.0, pos.1) {
                        self.selection_anchor = Some((pos.0, start_col));
                        self.selection_cursor = Some((pos.0, end_col));
                    } else {
                        self.selection_anchor = Some(pos);
                        self.selection_cursor = Some(pos);
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
            Hit::KeyFocus(_) | Hit::KeyFocusLost(_) => {
                self.draw_bg.redraw(cx);
            }
            Hit::KeyDown(e) => {
                if Self::is_clipboard_paste_shortcut(e.key_code, &e.modifiers) {
                    // The platform emits `TextInput { was_paste: true }` for this
                    // shortcut. Ignore keydown so we don't send an extra Ctrl+V
                    // control byte to the PTY before/after the pasted text.
                    return;
                }
                let sends_special_key = Self::is_special_pty_key(e.key_code);
                let sends_ctrl_char = e.modifiers.control && e.key_code.to_char(false).is_some();
                let sends_to_pty = sends_special_key || sends_ctrl_char;
                let is_enter = matches!(e.key_code, KeyCode::ReturnKey | KeyCode::NumpadEnter);
                if is_enter {
                    self.note_enter_pressed(cx);
                } else if sends_to_pty {
                    self.note_local_input(cx);
                }
                if sends_special_key {
                    self.send_key_to_pty(e.key_code, &e.modifiers);
                } else if sends_ctrl_char {
                    if let Some(c) = e.key_code.to_char(false) {
                        let s = c.to_string();
                        self.send_text_to_pty(&s, &e.modifiers);
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
