use crate::cell::{Color, StyleFlags};
use crate::color::{Palette, Rgb};
use crate::parser::{Action, Parser};
use crate::screen::{Cursor, CursorShape, Screen};

/// Terminal modes
#[derive(Clone, Debug, Default)]
pub struct TerminalMode {
    pub cursor_keys: bool,         // DECCKM: cursor keys send ESC O vs ESC [
    pub autowrap: bool,            // DECAWM: auto-wrap at right margin
    pub cursor_visible: bool,      // DECTCEM: cursor visible
    pub synchronized_update: bool, // DEC 2026: synchronized output mode
    pub alt_screen: bool,          // Alternate screen buffer active
    pub bracketed_paste: bool,     // Bracketed paste mode
    pub linefeed_newline: bool,    // LNM: LF also does CR
    pub origin: bool,              // DECOM: origin mode
    pub insert: bool,              // IRM: insert mode
    // Mouse modes
    pub mouse_tracking: MouseMode,
    pub mouse_sgr: bool,    // SGR extended mouse coordinates
    pub focus_events: bool, // Focus in/out reporting
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseMode {
    #[default]
    None,
    X10,         // Button press only
    Normal,      // Button press/release
    ButtonEvent, // + drag
    AnyEvent,    // + all motion
}

impl TerminalMode {
    fn new() -> Self {
        Self {
            autowrap: true,
            cursor_visible: true,
            ..Default::default()
        }
    }
}

pub struct Terminal {
    pub primary: Screen,
    pub alternate: Screen,
    pub active: ScreenKind,

    pub modes: TerminalMode,
    pub palette: Palette,
    pub default_fg: Rgb,
    pub default_bg: Rgb,
    pub cursor_color: Option<Rgb>,
    pub title: String,

    parser: Parser,
    actions_buf: Vec<Action>,
    outbound: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenKind {
    Primary,
    Alternate,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            primary: Screen::new(cols, rows, true),
            alternate: Screen::new(cols, rows, false),
            active: ScreenKind::Primary,
            modes: TerminalMode::new(),
            palette: Palette::default(),
            default_fg: Rgb::new(0xd9, 0xdc, 0xda),
            default_bg: Rgb::new(0x1d, 0x1f, 0x21),
            cursor_color: None,
            title: String::new(),
            parser: Parser::new(),
            actions_buf: Vec::with_capacity(64),
            outbound: Vec::with_capacity(64),
        }
    }

    pub fn screen(&self) -> &Screen {
        match self.active {
            ScreenKind::Primary => &self.primary,
            ScreenKind::Alternate => &self.alternate,
        }
    }

    pub fn screen_mut(&mut self) -> &mut Screen {
        match self.active {
            ScreenKind::Primary => &mut self.primary,
            ScreenKind::Alternate => &mut self.alternate,
        }
    }

    pub fn cursor(&self) -> &Cursor {
        &self.screen().cursor
    }

    pub fn cols(&self) -> usize {
        self.screen().cols()
    }

    pub fn rows(&self) -> usize {
        self.screen().rows()
    }

    /// Process raw bytes from PTY output.
    pub fn process_bytes(&mut self, bytes: &[u8]) {
        self.actions_buf.clear();
        self.parser.process(bytes, &mut self.actions_buf);

        // Process collected actions
        let actions: Vec<Action> = std::mem::take(&mut self.actions_buf);
        for action in &actions {
            self.handle_action(action);
        }
        self.actions_buf = actions;
    }

    /// Take terminal-generated reply bytes (DSR/DA/CPR/etc.) to send back to the PTY.
    pub fn take_outbound(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.outbound)
    }

    fn push_outbound(&mut self, bytes: &[u8]) {
        self.outbound.extend_from_slice(bytes);
    }

    fn handle_action(&mut self, action: &Action) {
        match action {
            Action::Print(c) => self.print(*c),
            Action::Execute(b) => self.execute(*b),
            Action::CsiDispatch { params, final_char } => {
                self.csi_dispatch(params, *final_char);
            }
            Action::EscDispatch {
                intermediates,
                intermediates_len,
                final_char,
            } => {
                self.esc_dispatch(&intermediates[..*intermediates_len], *final_char);
            }
            Action::OscDispatch { command } => {
                self.osc_dispatch(command);
            }
        }
    }

    fn print(&mut self, c: char) {
        if self.modes.insert {
            self.screen_mut().insert_blanks(1);
        }
        self.screen_mut().write_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => {
                // BEL — bell/notification
            }
            0x08 => {
                // BS — backspace
                self.screen_mut().do_backspace();
            }
            0x09 => {
                // HT — horizontal tab
                self.screen_mut().do_tab();
            }
            0x0a | 0x0b | 0x0c => {
                // LF, VT, FF — line feed
                self.screen_mut().do_linefeed();
                if self.modes.linefeed_newline {
                    self.screen_mut().do_carriage_return();
                }
            }
            0x0d => {
                // CR — carriage return
                self.screen_mut().do_carriage_return();
            }
            0x0e => {
                // SO — shift out (G1 charset) — ignore for now
            }
            0x0f => {
                // SI — shift in (G0 charset) — ignore for now
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &crate::parser::CsiParams, final_char: u8) {
        let is_private = params.has_intermediate(b'?');

        match final_char {
            // CUU — Cursor Up
            b'A' => {
                let n = params.get(0, 1) as usize;
                let screen = self.screen_mut();
                let new_y = screen.cursor.y.saturating_sub(n);
                let new_y = if screen.cursor.y >= screen.scroll_top {
                    new_y.max(screen.scroll_top)
                } else {
                    new_y
                };
                screen.cursor.y = new_y;
                screen.cursor.pending_wrap = false;
            }
            // CUD — Cursor Down
            b'B' => {
                let n = params.get(0, 1) as usize;
                let screen = self.screen_mut();
                let limit = if screen.cursor.y < screen.scroll_bottom {
                    screen.scroll_bottom - 1
                } else {
                    screen.rows() - 1
                };
                screen.cursor.y = (screen.cursor.y + n).min(limit);
                screen.cursor.pending_wrap = false;
            }
            // CUF — Cursor Forward (right)
            b'C' => {
                let n = params.get(0, 1) as usize;
                let screen = self.screen_mut();
                screen.cursor.x = (screen.cursor.x + n).min(screen.cols() - 1);
                screen.cursor.pending_wrap = false;
            }
            // CUB — Cursor Back (left)
            b'D' => {
                let n = params.get(0, 1) as usize;
                let screen = self.screen_mut();
                screen.cursor.x = screen.cursor.x.saturating_sub(n);
                screen.cursor.pending_wrap = false;
            }
            // CNL — Cursor Next Line
            b'E' => {
                let n = params.get(0, 1) as usize;
                let screen = self.screen_mut();
                let limit = screen.rows() - 1;
                screen.cursor.y = (screen.cursor.y + n).min(limit);
                screen.cursor.x = 0;
                screen.cursor.pending_wrap = false;
            }
            // CPL — Cursor Previous Line
            b'F' => {
                let n = params.get(0, 1) as usize;
                let screen = self.screen_mut();
                screen.cursor.y = screen.cursor.y.saturating_sub(n);
                screen.cursor.x = 0;
                screen.cursor.pending_wrap = false;
            }
            // CHA — Cursor Horizontal Absolute / HPA
            b'G' | b'`' => {
                let col = params.get(0, 1) as usize;
                let screen = self.screen_mut();
                screen.cursor.x = (col.saturating_sub(1)).min(screen.cols() - 1);
                screen.cursor.pending_wrap = false;
            }
            // CUP — Cursor Position / HVP
            b'H' | b'f' => {
                let row = params.get(0, 1) as usize;
                let col = params.get(1, 1) as usize;
                let screen = self.screen_mut();
                screen.move_cursor_to(col.saturating_sub(1), row.saturating_sub(1));
            }
            // ED — Erase in Display
            b'J' => {
                let mode = params.get(0, 0);
                self.screen_mut().erase_display(mode);
            }
            // EL — Erase in Line
            b'K' => {
                let mode = params.get(0, 0);
                self.screen_mut().erase_line(mode);
            }
            // IL — Insert Lines
            b'L' => {
                let n = params.get(0, 1) as usize;
                self.screen_mut().insert_lines(n);
            }
            // DL — Delete Lines
            b'M' => {
                let n = params.get(0, 1) as usize;
                self.screen_mut().delete_lines(n);
            }
            // DCH — Delete Characters
            b'P' => {
                let n = params.get(0, 1) as usize;
                self.screen_mut().delete_chars(n);
            }
            // SU — Scroll Up
            b'S' if !is_private => {
                let n = params.get(0, 1) as usize;
                self.screen_mut().scroll_up(n);
            }
            // SD — Scroll Down
            b'T' if !is_private => {
                let n = params.get(0, 1) as usize;
                self.screen_mut().scroll_down(n);
            }
            // ECH — Erase Characters
            b'X' => {
                let n = params.get(0, 1) as usize;
                self.screen_mut().erase_chars(n);
            }
            // ICH — Insert Blank Characters
            b'@' => {
                let n = params.get(0, 1) as usize;
                self.screen_mut().insert_blanks(n);
            }
            // VPA — Vertical Position Absolute
            b'd' => {
                let row = params.get(0, 1) as usize;
                let screen = self.screen_mut();
                screen.cursor.y = (row.saturating_sub(1)).min(screen.rows() - 1);
                screen.cursor.pending_wrap = false;
            }
            // SGR — Select Graphic Rendition
            b'm' => {
                self.handle_sgr(params);
            }
            // DSR — Device Status Report
            b'n' if !is_private => {
                match params.get(0, 0) {
                    5 => {
                        // Report terminal OK.
                        self.push_outbound(b"\x1b[0n");
                    }
                    6 => {
                        // CPR — Report cursor position (1-based row/col).
                        let screen = self.screen();
                        let row = screen.cursor.y + 1;
                        let col = screen.cursor.x + 1;
                        let reply = format!("\x1b[{};{}R", row, col);
                        self.push_outbound(reply.as_bytes());
                    }
                    _ => {}
                }
            }
            // DEC-specific DSR / DECXCPR
            b'n' if is_private => {
                if params.get(0, 0) == 6 {
                    let screen = self.screen();
                    let row = screen.cursor.y + 1;
                    let col = screen.cursor.x + 1;
                    let reply = format!("\x1b[?{};{}R", row, col);
                    self.push_outbound(reply.as_bytes());
                }
            }
            // DECSTBM — Set Top and Bottom Margins
            b'r' if !is_private => {
                let top = params.get(0, 1) as usize;
                let bottom = params.get(1, 0) as usize;
                self.screen_mut().set_scroll_region(top, bottom);
                // Move cursor to home
                self.screen_mut().move_cursor_to(0, 0);
            }
            // DECSC / SCOSC — Save Cursor Position
            b's' if !is_private => {
                self.screen_mut().save_cursor();
            }
            // DECRC / SCORC — Restore Cursor Position
            b'u' if !is_private => {
                self.screen_mut().restore_cursor();
            }
            // DA — Device Attributes
            b'c' if !is_private => {
                if params.has_intermediate(b'>') {
                    // Secondary DA.
                    self.push_outbound(b"\x1b[>0;0;0c");
                } else {
                    // Primary DA: VT100 with Advanced Video Option.
                    self.push_outbound(b"\x1b[?1;2c");
                }
            }
            // DECSET / DECRST — DEC Private Mode Set/Reset
            b'h' if is_private => {
                for i in 0..params.len {
                    self.set_dec_mode(params.params[i], true);
                }
            }
            b'l' if is_private => {
                for i in 0..params.len {
                    self.set_dec_mode(params.params[i], false);
                }
            }
            // SM — Set Mode (ANSI)
            b'h' if !is_private => {
                for i in 0..params.len {
                    self.set_ansi_mode(params.params[i], true);
                }
            }
            // RM — Reset Mode (ANSI)
            b'l' if !is_private => {
                for i in 0..params.len {
                    self.set_ansi_mode(params.params[i], false);
                }
            }
            // Cursor style (DECSCUSR)
            b'q' if params.has_intermediate(b' ') => {
                let shape = params.get(0, 0);
                self.screen_mut().cursor.shape = match shape {
                    0 | 1 => CursorShape::Block,
                    2 => CursorShape::Block,
                    3 | 4 => CursorShape::Underline,
                    5 | 6 => CursorShape::Bar,
                    _ => CursorShape::Block,
                };
            }
            _ => {
                // Unhandled CSI sequence — ignore
            }
        }
    }

    fn set_dec_mode(&mut self, mode: u16, enable: bool) {
        match mode {
            1 => self.modes.cursor_keys = enable, // DECCKM
            7 => self.modes.autowrap = enable,    // DECAWM
            12 => {
                // Cursor blink — we track visibility, not blink per se
            }
            25 => self.modes.cursor_visible = enable, // DECTCEM
            6 => {
                // DECOM — origin mode
                self.modes.origin = enable;
                self.screen_mut().move_cursor_to(0, 0);
            }
            47 => {
                // Alt screen buffer (no save cursor, no clear)
                self.switch_screen(enable, false, false);
            }
            1000 => {
                self.modes.mouse_tracking = if enable {
                    MouseMode::Normal
                } else {
                    MouseMode::None
                };
            }
            1002 => {
                self.modes.mouse_tracking = if enable {
                    MouseMode::ButtonEvent
                } else {
                    MouseMode::None
                };
            }
            1003 => {
                self.modes.mouse_tracking = if enable {
                    MouseMode::AnyEvent
                } else {
                    MouseMode::None
                };
            }
            1004 => self.modes.focus_events = enable,
            1006 => self.modes.mouse_sgr = enable,
            1049 => {
                // Alt screen buffer (save cursor + clear)
                self.switch_screen(enable, true, true);
            }
            2004 => self.modes.bracketed_paste = enable,
            2026 => self.modes.synchronized_update = enable,
            _ => {
                // Unknown DEC mode — ignore
            }
        }
    }

    fn set_ansi_mode(&mut self, mode: u16, enable: bool) {
        match mode {
            4 => self.modes.insert = enable,            // IRM
            20 => self.modes.linefeed_newline = enable, // LNM
            _ => {}
        }
    }

    fn switch_screen(&mut self, to_alt: bool, save_cursor: bool, clear: bool) {
        if to_alt && self.active == ScreenKind::Primary {
            if save_cursor {
                self.primary.save_cursor();
            }
            self.active = ScreenKind::Alternate;
            self.modes.alt_screen = true;
            if clear {
                self.alternate.erase_display(2);
                self.alternate.move_cursor_to(0, 0);
            }
        } else if !to_alt && self.active == ScreenKind::Alternate {
            self.active = ScreenKind::Primary;
            self.modes.alt_screen = false;
            if save_cursor {
                self.primary.restore_cursor();
            }
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], final_char: u8) {
        match (intermediates, final_char) {
            // DECSC — Save Cursor
            ([], b'7') => self.screen_mut().save_cursor(),
            // DECRC — Restore Cursor
            ([], b'8') => self.screen_mut().restore_cursor(),
            // RI — Reverse Index (cursor up, scroll if at top)
            ([], b'M') => {
                let screen = self.screen_mut();
                if screen.cursor.y == screen.scroll_top {
                    screen.scroll_down(1);
                } else if screen.cursor.y > 0 {
                    screen.cursor.y -= 1;
                }
                screen.cursor.pending_wrap = false;
            }
            // IND — Index (cursor down, scroll if at bottom)
            ([], b'D') => {
                self.screen_mut().do_linefeed();
            }
            // NEL — Next Line
            ([], b'E') => {
                self.screen_mut().do_linefeed();
                self.screen_mut().do_carriage_return();
            }
            // HTS — Horizontal Tab Set
            ([], b'H') => {
                let x = self.screen().cursor.x;
                if x < self.screen().cols() {
                    self.screen_mut().tabstops[x] = true;
                }
            }
            // RIS — Reset to Initial State
            ([], b'c') => {
                let cols = self.cols();
                let rows = self.rows();
                *self = Terminal::new(cols, rows);
            }
            _ => {
                // Unhandled ESC sequence
            }
        }
    }

    fn osc_dispatch(&mut self, command: &[u8]) {
        // OSC format: "N;data" where N is the command number
        let s = match std::str::from_utf8(command) {
            Ok(s) => s,
            Err(_) => return,
        };

        let (num_str, data) = match s.find(';') {
            Some(pos) => (&s[..pos], &s[pos + 1..]),
            None => (s, ""),
        };

        let num: u16 = match num_str.parse() {
            Ok(n) => n,
            Err(_) => return,
        };

        match num {
            // Window title
            0 | 2 => {
                self.title = data.to_string();
            }
            // Icon name (ignore, we use title)
            1 => {}
            // Set color palette entries: "index;spec(;index;spec...)"
            4 => {
                let mut parts = data.split(';');
                while let (Some(idx_str), Some(spec)) = (parts.next(), parts.next()) {
                    let Ok(idx) = idx_str.parse::<usize>() else {
                        continue;
                    };
                    if idx >= 256 {
                        continue;
                    }
                    if let Some(rgb) = Self::parse_osc_color(spec) {
                        self.palette.colors[idx] = rgb;
                    }
                }
            }
            // Default foreground/background/cursor colors.
            // "?" queries the current value; otherwise set the color.
            10 => {
                if data == "?" {
                    let c = self.default_fg;
                    let reply = format!(
                        "\x1b]10;rgb:{:04x}/{:04x}/{:04x}\x1b\\",
                        c.r as u16 * 257,
                        c.g as u16 * 257,
                        c.b as u16 * 257
                    );
                    self.push_outbound(reply.as_bytes());
                } else if let Some(rgb) = Self::parse_osc_color(data) {
                    self.default_fg = rgb;
                }
            }
            11 => {
                if data == "?" {
                    let c = self.default_bg;
                    let reply = format!(
                        "\x1b]11;rgb:{:04x}/{:04x}/{:04x}\x1b\\",
                        c.r as u16 * 257,
                        c.g as u16 * 257,
                        c.b as u16 * 257
                    );
                    self.push_outbound(reply.as_bytes());
                } else if let Some(rgb) = Self::parse_osc_color(data) {
                    self.default_bg = rgb;
                }
            }
            12 => {
                if data == "?" {
                    let c = self.cursor_color.unwrap_or(self.default_fg);
                    let reply = format!(
                        "\x1b]12;rgb:{:04x}/{:04x}/{:04x}\x1b\\",
                        c.r as u16 * 257,
                        c.g as u16 * 257,
                        c.b as u16 * 257
                    );
                    self.push_outbound(reply.as_bytes());
                } else {
                    self.cursor_color = Self::parse_osc_color(data);
                }
            }
            // Reset palette entries (empty means all).
            104 => {
                let defaults = Palette::default_palette();
                if data.trim().is_empty() {
                    self.palette = defaults;
                } else {
                    for idx_str in data.split(';') {
                        let Ok(idx) = idx_str.parse::<usize>() else {
                            continue;
                        };
                        if idx < 256 {
                            self.palette.colors[idx] = defaults.colors[idx];
                        }
                    }
                }
            }
            // Reset default fg/bg/cursor colors.
            110 => self.default_fg = Rgb::new(0xd9, 0xdc, 0xda),
            111 => self.default_bg = Rgb::new(0x1d, 0x1f, 0x21),
            112 => self.cursor_color = None,
            _ => {
                // Other OSC — ignore for now
            }
        }
    }

    fn parse_osc_color(spec: &str) -> Option<Rgb> {
        if let Some(hex) = spec.strip_prefix('#') {
            if hex.len() == 6 {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                return Some(Rgb::new(r, g, b));
            }
            return None;
        }
        if let Some(rest) = spec.strip_prefix("rgb:") {
            let mut it = rest.split('/');
            let r = Self::parse_osc_hex_component(it.next()?)?;
            let g = Self::parse_osc_hex_component(it.next()?)?;
            let b = Self::parse_osc_hex_component(it.next()?)?;
            return Some(Rgb::new(r, g, b));
        }
        None
    }

    fn parse_osc_hex_component(comp: &str) -> Option<u8> {
        if comp.is_empty() || comp.len() > 4 {
            return None;
        }
        let value = u16::from_str_radix(comp, 16).ok()?;
        let max = (1u32 << (comp.len() as u32 * 4)) - 1;
        if max == 0 {
            return None;
        }
        Some(((value as u32 * 255) / max) as u8)
    }

    fn parse_sgr_extended_color(
        params: &crate::parser::CsiParams,
        i: usize,
    ) -> Option<(Color, usize)> {
        let mode_idx = i + 1;
        if mode_idx >= params.len {
            return None;
        }
        match params.params[mode_idx] {
            5 => {
                let idx_idx = mode_idx + 1;
                if idx_idx < params.len {
                    Some((
                        Color::Palette(params.params[idx_idx].min(255) as u8),
                        idx_idx,
                    ))
                } else {
                    None
                }
            }
            2 => {
                // Colon form may include a color-space id before RGB, e.g. 38:2::R:G:B.
                let rgb_start = if params.has_colon && mode_idx + 4 < params.len {
                    let color_space = params.params[mode_idx + 1];
                    if color_space <= 1 {
                        mode_idx + 2
                    } else {
                        mode_idx + 1
                    }
                } else {
                    mode_idx + 1
                };
                let b_idx = rgb_start + 2;
                if b_idx < params.len {
                    Some((
                        Color::Rgb(
                            params.params[rgb_start].min(255) as u8,
                            params.params[rgb_start + 1].min(255) as u8,
                            params.params[b_idx].min(255) as u8,
                        ),
                        b_idx,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn handle_sgr(&mut self, params: &crate::parser::CsiParams) {
        if params.len == 0 {
            // SGR with no params = reset
            self.screen_mut().cursor.style.reset();
            return;
        }

        let mut i = 0;
        while i < params.len {
            let p = params.params[i];
            match p {
                0 => self.screen_mut().cursor.style.reset(),
                1 => self.screen_mut().cursor.style.flags.set(StyleFlags::BOLD),
                2 => self.screen_mut().cursor.style.flags.set(StyleFlags::FAINT),
                3 => self.screen_mut().cursor.style.flags.set(StyleFlags::ITALIC),
                4 => self.screen_mut().cursor.style.flags.set_underline(1),
                5 | 6 => self.screen_mut().cursor.style.flags.set(StyleFlags::BLINK),
                7 => self
                    .screen_mut()
                    .cursor
                    .style
                    .flags
                    .set(StyleFlags::INVERSE),
                8 => self
                    .screen_mut()
                    .cursor
                    .style
                    .flags
                    .set(StyleFlags::INVISIBLE),
                9 => self
                    .screen_mut()
                    .cursor
                    .style
                    .flags
                    .set(StyleFlags::STRIKETHROUGH),
                21 => self.screen_mut().cursor.style.flags.set_underline(2), // double underline
                22 => {
                    self.screen_mut().cursor.style.flags.clear(StyleFlags::BOLD);
                    self.screen_mut()
                        .cursor
                        .style
                        .flags
                        .clear(StyleFlags::FAINT);
                }
                23 => self
                    .screen_mut()
                    .cursor
                    .style
                    .flags
                    .clear(StyleFlags::ITALIC),
                24 => self.screen_mut().cursor.style.flags.set_underline(0),
                25 => self
                    .screen_mut()
                    .cursor
                    .style
                    .flags
                    .clear(StyleFlags::BLINK),
                27 => self
                    .screen_mut()
                    .cursor
                    .style
                    .flags
                    .clear(StyleFlags::INVERSE),
                28 => self
                    .screen_mut()
                    .cursor
                    .style
                    .flags
                    .clear(StyleFlags::INVISIBLE),
                29 => self
                    .screen_mut()
                    .cursor
                    .style
                    .flags
                    .clear(StyleFlags::STRIKETHROUGH),
                // Standard foreground colors (30-37)
                30..=37 => {
                    self.screen_mut().cursor.style.fg = Color::Palette((p - 30) as u8);
                }
                // Default foreground
                39 => self.screen_mut().cursor.style.fg = Color::Default,
                // Standard background colors (40-47)
                40..=47 => {
                    self.screen_mut().cursor.style.bg = Color::Palette((p - 40) as u8);
                }
                // Default background
                49 => self.screen_mut().cursor.style.bg = Color::Default,
                // Extended foreground (38;5;N or 38;2;R;G;B)
                38 => {
                    if let Some((fg, new_i)) = Self::parse_sgr_extended_color(params, i) {
                        self.screen_mut().cursor.style.fg = fg;
                        i = new_i;
                    }
                }
                // Extended background (48;5;N or 48;2;R;G;B)
                48 => {
                    if let Some((bg, new_i)) = Self::parse_sgr_extended_color(params, i) {
                        self.screen_mut().cursor.style.bg = bg;
                        i = new_i;
                    }
                }
                // Extended underline color (58;...)
                58 => {
                    if let Some((_ul, new_i)) = Self::parse_sgr_extended_color(params, i) {
                        // Underline color is not stored yet; consume params to keep parsing in sync.
                        i = new_i;
                    }
                }
                // Reset underline color (ignored for now).
                59 => {}
                // Bright foreground (90-97)
                90..=97 => {
                    self.screen_mut().cursor.style.fg = Color::Palette((p - 90 + 8) as u8);
                }
                // Bright background (100-107)
                100..=107 => {
                    self.screen_mut().cursor.style.bg = Color::Palette((p - 100 + 8) as u8);
                }
                53 => self
                    .screen_mut()
                    .cursor
                    .style
                    .flags
                    .set(StyleFlags::OVERLINE),
                55 => self
                    .screen_mut()
                    .cursor
                    .style
                    .flags
                    .clear(StyleFlags::OVERLINE),
                _ => {} // Unknown SGR — ignore
            }
            i += 1;
        }
    }

    /// Resize the terminal
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.primary.resize(cols, rows);
        self.alternate.resize(cols, rows);
    }

    /// Encode a key event as bytes to send to the PTY.
    /// Returns the byte sequence, or None if the key isn't handled.
    pub fn encode_key(
        &self,
        key_code: KeyCode,
        text: &str,
        shift: bool,
        ctrl: bool,
        alt: bool,
    ) -> Option<Vec<u8>> {
        // For text input, handle Ctrl+key and Alt+key
        if !text.is_empty() && key_code == KeyCode::None {
            if ctrl {
                // Ctrl+A..Z maps to 0x01..0x1a
                let c = text.chars().next()?;
                if c >= 'a' && c <= 'z' {
                    return Some(vec![(c as u8) - b'a' + 1]);
                }
                if c >= 'A' && c <= 'Z' {
                    return Some(vec![(c as u8) - b'A' + 1]);
                }
            }
            if alt {
                let mut bytes = vec![0x1b];
                bytes.extend_from_slice(text.as_bytes());
                return Some(bytes);
            }
            return Some(text.as_bytes().to_vec());
        }

        // Special keys
        let modifier = modifier_param(shift, ctrl, alt);

        match key_code {
            KeyCode::Return => Some(vec![0x0d]),
            KeyCode::Tab => {
                if shift {
                    Some(b"\x1b[Z".to_vec())
                } else {
                    Some(vec![0x09])
                }
            }
            KeyCode::Backspace => {
                if alt {
                    Some(vec![0x1b, 0x7f])
                } else {
                    Some(vec![0x7f])
                }
            }
            KeyCode::Escape => Some(vec![0x1b]),
            KeyCode::Delete => {
                if modifier > 0 {
                    Some(format!("\x1b[3;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[3~".to_vec())
                }
            }
            KeyCode::Up => Some(cursor_key(b'A', modifier, self.modes.cursor_keys)),
            KeyCode::Down => Some(cursor_key(b'B', modifier, self.modes.cursor_keys)),
            KeyCode::Right => Some(cursor_key(b'C', modifier, self.modes.cursor_keys)),
            KeyCode::Left => Some(cursor_key(b'D', modifier, self.modes.cursor_keys)),
            KeyCode::Home => Some(cursor_key(b'H', modifier, self.modes.cursor_keys)),
            KeyCode::End => Some(cursor_key(b'F', modifier, self.modes.cursor_keys)),
            KeyCode::PageUp => {
                if modifier > 0 {
                    Some(format!("\x1b[5;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[5~".to_vec())
                }
            }
            KeyCode::PageDown => {
                if modifier > 0 {
                    Some(format!("\x1b[6;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[6~".to_vec())
                }
            }
            KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
            KeyCode::F1 => Some(func_key(b'P', 11, modifier)),
            KeyCode::F2 => Some(func_key(b'Q', 12, modifier)),
            KeyCode::F3 => Some(func_key(b'R', 13, modifier)),
            KeyCode::F4 => Some(func_key(b'S', 14, modifier)),
            KeyCode::F5 => {
                if modifier > 0 {
                    Some(format!("\x1b[15;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[15~".to_vec())
                }
            }
            KeyCode::F6 => {
                if modifier > 0 {
                    Some(format!("\x1b[17;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[17~".to_vec())
                }
            }
            KeyCode::F7 => {
                if modifier > 0 {
                    Some(format!("\x1b[18;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[18~".to_vec())
                }
            }
            KeyCode::F8 => {
                if modifier > 0 {
                    Some(format!("\x1b[19;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[19~".to_vec())
                }
            }
            KeyCode::F9 => {
                if modifier > 0 {
                    Some(format!("\x1b[20;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[20~".to_vec())
                }
            }
            KeyCode::F10 => {
                if modifier > 0 {
                    Some(format!("\x1b[21;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[21~".to_vec())
                }
            }
            KeyCode::F11 => {
                if modifier > 0 {
                    Some(format!("\x1b[23;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[23~".to_vec())
                }
            }
            KeyCode::F12 => {
                if modifier > 0 {
                    Some(format!("\x1b[24;{}~", modifier).into_bytes())
                } else {
                    Some(b"\x1b[24~".to_vec())
                }
            }
            _ => None,
        }
    }
}

/// Key codes that encode_key understands (maps to platform key events)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    None,
    Return,
    Tab,
    Backspace,
    Escape,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

fn modifier_param(shift: bool, ctrl: bool, alt: bool) -> u8 {
    let mut m = 0u8;
    if shift {
        m |= 1;
    }
    if alt {
        m |= 2;
    }
    if ctrl {
        m |= 4;
    }
    if m > 0 {
        m + 1
    } else {
        0
    }
}

fn cursor_key(ch: u8, modifier: u8, app_cursor: bool) -> Vec<u8> {
    if modifier > 0 {
        format!("\x1b[1;{}{}", modifier, ch as char).into_bytes()
    } else if app_cursor {
        vec![0x1b, b'O', ch]
    } else {
        vec![0x1b, b'[', ch]
    }
}

fn func_key(ss3_char: u8, csi_num: u16, modifier: u8) -> Vec<u8> {
    if modifier > 0 {
        format!("\x1b[{};{}~", csi_num, modifier).into_bytes()
    } else {
        vec![0x1b, b'O', ss3_char]
    }
}

#[cfg(test)]
mod tests {
    use super::{Color, Terminal};

    fn write_line(screen: &mut crate::screen::Screen, ch: char) {
        screen.write_char(ch);
        screen.do_linefeed();
        screen.do_carriage_return();
    }

    #[test]
    fn dsr_cursor_position_reply() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process_bytes(b"\x1b[12;34H");
        terminal.process_bytes(b"\x1b[6n");
        assert_eq!(terminal.take_outbound(), b"\x1b[12;34R".to_vec());
    }

    #[test]
    fn da_primary_reply() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process_bytes(b"\x1b[c");
        assert_eq!(terminal.take_outbound(), b"\x1b[?1;2c".to_vec());
    }

    #[test]
    fn sgr_truecolor_applies_to_cell_style() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process_bytes(b"\x1b[38;2;10;20;30mX");
        let cell = terminal.screen().grid.cell(0, 0);
        assert_eq!(cell.style.fg, Color::Rgb(10, 20, 30));
    }

    #[test]
    fn osc_updates_default_colors() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process_bytes(b"\x1b]10;#112233\x07");
        terminal.process_bytes(b"\x1b]11;rgb:44/55/66\x07");
        assert_eq!(terminal.default_fg.r, 0x11);
        assert_eq!(terminal.default_fg.g, 0x22);
        assert_eq!(terminal.default_fg.b, 0x33);
        assert_eq!(terminal.default_bg.r, 0x44);
        assert_eq!(terminal.default_bg.g, 0x55);
        assert_eq!(terminal.default_bg.b, 0x66);
    }

    #[test]
    fn osc_with_st_terminator_is_applied() {
        let mut terminal = Terminal::new(80, 24);
        terminal.process_bytes(b"\x1b]10;#abcdef\x1b\\");
        assert_eq!(terminal.default_fg.r, 0xab);
        assert_eq!(terminal.default_fg.g, 0xcd);
        assert_eq!(terminal.default_fg.b, 0xef);
    }

    #[test]
    fn resize_grow_pulls_scrollback_if_at_bottom() {
        let mut terminal = Terminal::new(4, 3);
        write_line(&mut terminal.primary, 'A');
        write_line(&mut terminal.primary, 'B');
        write_line(&mut terminal.primary, 'C');
        write_line(&mut terminal.primary, 'D');
        terminal.primary.write_char('E');

        // A and B scrolled off; C D E visible at rows 0-2.
        assert_eq!(terminal.primary.scrollback_len(), 2);
        terminal.resize(4, 5);

        let screen = terminal.screen();
        // Cursor was at the bottom, so scrollback is pulled into the grid.
        assert_eq!(screen.grid.cell(0, 0).codepoint, 'A');
        assert_eq!(screen.grid.cell(0, 1).codepoint, 'B');
        assert_eq!(screen.grid.cell(0, 2).codepoint, 'C');
        assert_eq!(screen.grid.cell(0, 3).codepoint, 'D');
        assert_eq!(screen.grid.cell(0, 4).codepoint, 'E');
        // Scrollback consumed
        assert_eq!(screen.scrollback_len(), 0);
        // Cursor moves down
        assert_eq!(screen.cursor.y, 4);
    }

    /// On grow, scrollback is pulled to keep content anchored to the bottom
    /// (matching macOS Terminal).  The TUI app receives SIGWINCH and redraws.
    #[test]

    #[test]
    fn resize_shrink_keeps_cursor_row_when_already_visible() {
        let mut terminal = Terminal::new(4, 5);
        write_line(&mut terminal.primary, 'A');
        write_line(&mut terminal.primary, 'B');
        write_line(&mut terminal.primary, 'C');
        write_line(&mut terminal.primary, 'D');
        terminal.primary.write_char('E');

        // Cursor is already in the future 0..3 viewport, so shrinking should
        // trim bottom rows instead of pushing top rows into scrollback.
        terminal.primary.move_cursor_to(0, 1);
        terminal.resize(4, 3);

        let screen = terminal.screen();
        assert_eq!(screen.grid.cell(0, 0).codepoint, 'A');
        assert_eq!(screen.grid.cell(0, 1).codepoint, 'B');
        assert_eq!(screen.grid.cell(0, 2).codepoint, 'C');
        assert_eq!(screen.scrollback_len(), 0);
        assert_eq!(screen.cursor.y, 1);
    }

    #[test]
    fn resize_grow_pulls_scrollback_when_screen_was_full() {
        let mut terminal = Terminal::new(4, 3);
        write_line(&mut terminal.primary, 'A');
        write_line(&mut terminal.primary, 'B');
        write_line(&mut terminal.primary, 'C');
        write_line(&mut terminal.primary, 'D');
        terminal.primary.write_char('E');

        // A and B scrolled off; C D E visible at rows 0-2.
        assert_eq!(terminal.primary.scrollback_len(), 2);

        // Move cursor to top (TUI apps can place cursor anywhere)
        terminal.primary.move_cursor_to(0, 0);

        terminal.resize(4, 5);

        let screen = terminal.screen();
        // Screen was full, so scrollback is pulled regardless of cursor position.
        assert_eq!(screen.grid.cell(0, 0).codepoint, 'A');
        assert_eq!(screen.grid.cell(0, 1).codepoint, 'B');
        assert_eq!(screen.grid.cell(0, 2).codepoint, 'C');
        assert_eq!(screen.grid.cell(0, 3).codepoint, 'D');
        assert_eq!(screen.grid.cell(0, 4).codepoint, 'E');
        // Scrollback consumed
        assert_eq!(screen.scrollback_len(), 0);
        // Cursor shifted down by pulled count
        assert_eq!(screen.cursor.y, 2);
    }

    // ---------------------------------------------------------------
    // Codex-like TUI simulation tests
    // ---------------------------------------------------------------

    /// Helper: creates a terminal with shell output that scrolls (creating
    /// scrollback), then draws a Codex-like TUI using cursor positioning.
    /// Returns the terminal with cursor at the input prompt (middle of screen).
    fn setup_codex_tui(cols: u16, rows: u16) -> Terminal {
        let mut t = Terminal::new(cols as usize, rows as usize);

        // Phase 1: shell output fills the screen and creates scrollback.
        for i in 0..60u32 {
            let line = format!("shell line {}\r\n", i);
            t.process_bytes(line.as_bytes());
        }

        // Phase 2: Codex draws its TUI with cursor positioning.
        // Set scroll region for content area (rows 3 to rows-2, 1-based).
        t.process_bytes(format!("\x1b[3;{}r", rows - 2).as_bytes());

        // Header on row 1 (1-based) with blue bg.
        t.process_bytes(b"\x1b[1;1H\x1b[44m");
        let hdr = format!("{:<width$}", "=== Codex ===", width = cols as usize);
        t.process_bytes(hdr.as_bytes());
        t.process_bytes(b"\x1b[0m");

        // Separator on row 2.
        t.process_bytes(b"\x1b[2;1H");
        for _ in 0..cols {
            t.process_bytes(b"-");
        }

        // Content in the scroll region.
        for r in 3..=(rows - 2) {
            t.process_bytes(format!("\x1b[{};1H", r).as_bytes());
            let content = format!("content {}", r);
            t.process_bytes(content.as_bytes());
        }

        // Input prompt on row rows-1 (1-based).
        t.process_bytes(format!("\x1b[{};1H> ", rows - 1).as_bytes());

        // Status bar on row rows (1-based) with green bg.
        t.process_bytes(format!("\x1b[{};1H\x1b[42m", rows).as_bytes());
        let status = format!("{:<width$}", "status: ok", width = cols as usize);
        t.process_bytes(status.as_bytes());
        t.process_bytes(b"\x1b[0m");

        // Cursor back to input prompt area.
        t.process_bytes(format!("\x1b[{};3H", rows - 1).as_bytes());

        t
    }

    /// Helper: read a grid row as a trimmed string.
    fn grid_row_text(terminal: &Terminal, row: usize) -> String {
        let screen = terminal.screen();
        let cols = screen.cols();
        let mut s = String::new();
        for col in 0..cols {
            s.push(screen.grid.cell(col, row).codepoint);
        }
        s.trim_end().to_string()
    }

    fn snapshot_grid(terminal: &Terminal) -> Vec<Vec<char>> {
        let screen = terminal.screen();
        let rows = screen.rows();
        let cols = screen.cols();
        let mut out = Vec::with_capacity(rows);
        for row in 0..rows {
            let mut line = Vec::with_capacity(cols);
            for col in 0..cols {
                line.push(screen.grid.cell(col, row).codepoint);
            }
            out.push(line);
        }
        out
    }

    fn setup_makepad_tui_like_with_hi(cols: u16, rows: u16) -> Terminal {
        let mut t = Terminal::new(cols as usize, rows as usize);

        // Seed deep pre-TUI history with ls-like output so resize behavior is
        // exercised against realistic prior content (as in a shell session).
        for i in 0..220u32 {
            let line = format!(
                "-rw-r--r--  1 user  staff  {:>6} Mar  3 12:{:02} file_{:03}.txt\r\n",
                1024 + i,
                i % 60,
                i
            );
            t.process_bytes(line.as_bytes());
        }

        let scroll_bottom = rows - 4;
        t.process_bytes(format!("\x1b[1;{}r", scroll_bottom).as_bytes());

        // Header block in scroll region.
        t.process_bytes(b"\x1b[1;1H+--------------------------------------------+");
        t.process_bytes(b"\x1b[2;1H| >_ TUI Test (v0.1.0)                     |");
        t.process_bytes(b"\x1b[3;1H|                                            |");
        t.process_bytes(b"\x1b[4;1H| model: fake-model /model to change       |");
        t.process_bytes(b"\x1b[5;1H| directory: ~/makepad                     |");
        t.process_bytes(b"\x1b[6;1H+--------------------------------------------+");

        // Simulate "hi" + 10 streamed responses in the scroll region.
        t.process_bytes(format!("\x1b[{};1H", scroll_bottom).as_bytes());
        t.process_bytes(b"\x1b[K> hi\n");
        for i in 1..=10 {
            let line = format!("\x1b[K[{}/10] hi\n", i);
            t.process_bytes(line.as_bytes());
        }

        // Pinned prompt/status area.
        let prompt_row = rows - 1;
        let status_row = rows;
        t.process_bytes(format!("\x1b[{};1H\x1b[K> ", prompt_row).as_bytes());
        t.process_bytes(format!("\x1b[{};1H\x1b[Kfake-model - 100% left\n", status_row).as_bytes());
        t.process_bytes(format!("\x1b[{};3H", prompt_row).as_bytes());
        t
    }

    fn render_like_tui_resize(terminal: &mut Terminal, rows: u16) {
        let scroll_bottom = rows - 4;
        let working_row = rows - 3;
        let prompt_row = rows - 2;
        let blank_row = rows - 1;
        let status_row = rows;

        terminal.process_bytes(b"\x1b[r");
        terminal.process_bytes(format!("\x1b[{working_row};1H\x1b[K").as_bytes());
        terminal.process_bytes(format!("\x1b[{prompt_row};1H\x1b[K> ").as_bytes());
        terminal.process_bytes(format!("\x1b[{blank_row};1H\x1b[K").as_bytes());
        terminal.process_bytes(
            format!("\x1b[{status_row};1H\x1b[Kfake-model - 100% left").as_bytes(),
        );
        terminal.process_bytes(format!("\x1b[1;{scroll_bottom}r").as_bytes());
        terminal.process_bytes(format!("\x1b[{prompt_row};3H").as_bytes());
    }

    fn render_like_codex_resize(terminal: &mut Terminal, cols: u16, rows: u16) {
        // Codex-style redraw on SIGWINCH: re-establish region and fixed rows.
        terminal.process_bytes(format!("\x1b[3;{}r", rows - 2).as_bytes());

        terminal.process_bytes(b"\x1b[1;1H\x1b[K");
        let hdr = format!("{:<width$}", "=== Codex ===", width = cols as usize);
        terminal.process_bytes(hdr.as_bytes());

        terminal.process_bytes(b"\x1b[2;1H\x1b[K");
        let sep = "-".repeat(cols as usize);
        terminal.process_bytes(sep.as_bytes());

        for r in 3..=(rows - 2) {
            terminal.process_bytes(format!("\x1b[{};1H\x1b[K", r).as_bytes());
            let content = format!("content {}", r);
            terminal.process_bytes(content.as_bytes());
        }

        terminal.process_bytes(format!("\x1b[{};1H\x1b[K> ", rows - 1).as_bytes());
        terminal.process_bytes(format!("\x1b[{};1H\x1b[K", rows).as_bytes());
        let status = format!("{:<width$}", "status: ok", width = cols as usize);
        terminal.process_bytes(status.as_bytes());
        terminal.process_bytes(format!("\x1b[{};3H", rows - 1).as_bytes());
    }

    fn assert_codex_frame_invariants(terminal: &Terminal, cols: u16, rows: u16) {
        let screen = terminal.screen();
        let row0 = grid_row_text(terminal, 0);
        assert!(
            row0.starts_with("=== Codex ==="),
            "header row corrupted: '{}'",
            row0
        );
        let row1 = grid_row_text(terminal, 1);
        assert!(
            row1.starts_with("---"),
            "separator row corrupted: '{}'",
            row1
        );
        let prompt_row = rows as usize - 2;
        let status_row = rows as usize - 1;
        let prompt = grid_row_text(terminal, prompt_row);
        let status = grid_row_text(terminal, status_row);
        assert!(prompt.starts_with(">"), "prompt row corrupted at {}: '{}'", prompt_row, prompt);
        assert!(
            status.starts_with("status: ok"),
            "status row corrupted at {}: '{}'",
            status_row,
            status
        );
        assert_eq!(screen.cols(), cols as usize);
        assert_eq!(screen.rows(), rows as usize);
        assert_eq!(screen.cursor.y, prompt_row);
        assert_eq!(screen.cursor.x, 2);
    }

    #[test]
    fn codex_tui_grid_positions_stable_after_width_resize() {
        let mut t = setup_codex_tui(80, 24);

        // Verify initial layout.
        assert!(grid_row_text(&t, 0).starts_with("=== Codex ==="));
        assert!(grid_row_text(&t, 1).starts_with("---"));
        assert!(grid_row_text(&t, 2).starts_with("content 3"));
        assert!(grid_row_text(&t, 22).starts_with("> "));
        assert!(grid_row_text(&t, 23).starts_with("status: ok"));
        // Cursor at input prompt.
        assert_eq!(t.screen().cursor.y, 22); // row 23 is 0-based 22

        let sb_before = t.primary.scrollback_len();

        // Resize wider — only cols change.
        t.resize(100, 24);

        // Grid content must stay at the SAME row positions.
        assert!(grid_row_text(&t, 0).starts_with("=== Codex ==="),
            "row 0 after resize: '{}'", grid_row_text(&t, 0));
        assert!(grid_row_text(&t, 1).starts_with("---"),
            "row 1 after resize: '{}'", grid_row_text(&t, 1));
        assert!(grid_row_text(&t, 22).starts_with("> "),
            "row 22 after resize: '{}'", grid_row_text(&t, 22));
        assert!(grid_row_text(&t, 23).starts_with("status: ok"),
            "row 23 after resize: '{}'", grid_row_text(&t, 23));

        // Cursor y must not shift.
        assert_eq!(t.screen().cursor.y, 22);

        // used_rows must cover the full screen (content drawn to bottom).
        assert_eq!(t.screen().used_rows(), 24);

        // Scrollback may change size (reflow) but must not be pulled into grid.
        assert!(t.primary.scrollback_len() <= sb_before,
            "scrollback should not grow: was {}, now {}", sb_before, t.primary.scrollback_len());
    }

    #[test]

    #[test]
    fn codex_tui_grid_positions_stable_after_narrower_resize() {
        let mut t = setup_codex_tui(80, 24);

        // Resize narrower.
        t.resize(60, 24);

        // Grid content must stay at same rows (truncated horizontally, not shifted).
        assert!(grid_row_text(&t, 0).starts_with("=== Codex ==="),
            "row 0: '{}'", grid_row_text(&t, 0));
        assert!(grid_row_text(&t, 22).starts_with("> "),
            "row 22: '{}'", grid_row_text(&t, 22));
        // Status bar text truncated to 60 cols but still on row 23.
        assert!(grid_row_text(&t, 23).starts_with("status: ok"),
            "row 23: '{}'", grid_row_text(&t, 23));

        assert_eq!(t.screen().cursor.y, 22);
        assert_eq!(t.screen().used_rows(), 24);
    }

    #[test]
    fn codex_tui_used_rows_reflects_content_not_cursor() {
        let mut t = setup_codex_tui(80, 24);

        // Cursor is at row 22 (input prompt), but content extends to row 23.
        assert_eq!(t.screen().cursor.y, 22);
        assert_eq!(t.screen().used_rows(), 24); // content on all 24 rows

        // After resize, used_rows must still cover all content, not snap to cursor.
        t.resize(100, 24);
        assert!(t.screen().used_rows() >= 24,
            "used_rows={} should be >= 24 after width resize", t.screen().used_rows());

        // The viewport total (scrollback + used_rows) should be enough to scroll to bottom.
        let total = t.primary.scrollback_len() + t.screen().used_rows();
        assert!(total >= 24,
            "total_lines={} should be >= viewport rows", total);
    }

    #[test]
    fn codex_tui_styles_preserved_after_reflow() {
        let mut t = setup_codex_tui(80, 24);

        // Row 0 has blue bg (header).
        let header_bg = t.screen().grid.cell(0, 0).style.bg;
        assert_ne!(header_bg, Color::Default, "header should have colored bg");

        // Row 23 has green bg (status bar).
        let status_bg = t.screen().grid.cell(0, 23).style.bg;
        assert_ne!(status_bg, Color::Default, "status should have colored bg");

        // Resize wider.
        t.resize(100, 24);

        // Content cells should preserve their styles.
        let new_header_bg = t.screen().grid.cell(0, 0).style.bg;
        assert_eq!(new_header_bg, header_bg,
            "header bg should be preserved: {:?} vs {:?}", new_header_bg, header_bg);

        let new_status_bg = t.screen().grid.cell(0, 23).style.bg;
        assert_eq!(new_status_bg, status_bg,
            "status bg should be preserved: {:?} vs {:?}", new_status_bg, status_bg);
    }

    #[test]
    fn scrollback_reflows_when_width_changes() {
        let mut t = Terminal::new(20, 5);

        // Write a long line that wraps at 20 cols.
        let long_line = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"; // 36 chars
        for ch in long_line.chars() {
            t.primary.write_char(ch);
        }
        t.primary.do_linefeed();
        t.primary.do_carriage_return();
        t.primary.write_char('X');

        // At 20 cols, "ABCDE..." wraps into 2 rows. Then newline, then X.
        // Row 0: ABCDEFGHIJKLMNOPQRST (wrapped)
        // Row 1: UVWXYZ0123456789     (not wrapped)
        // Row 2: X
        assert_eq!(t.screen().grid.cell(0, 0).codepoint, 'A');
        assert!(t.screen().grid.line_wrapped[0]); // first row is wrapped

        // Scroll the content off into scrollback so we can test scrollback reflow.
        for _ in 0..5 {
            t.primary.do_linefeed();
            t.primary.do_carriage_return();
        }
        // Now those rows are in scrollback.
        assert!(t.primary.scrollback_len() >= 2);

        let sb = t.primary.scrollback();
        // First scrollback row should be the wrapped 'A..T' row.
        assert_eq!(sb[0][0].codepoint, 'A');

        // Resize wider — the wrapped line should unwrap in scrollback.
        t.resize(40, 5);

        let sb = t.primary.scrollback();
        // The 36-char line should now fit in one 40-col row in scrollback.
        assert_eq!(sb.len(), t.primary.scrollback_len());
        // Find the row with 'A'.
        let a_row = sb.iter().position(|row| row[0].codepoint == 'A').unwrap();
        assert_eq!(sb[a_row][35].codepoint, '9', "full line should be in one row");
        // Should NOT be wrapped anymore.
        assert!(!t.primary.scrollback_wrapped[a_row],
            "unwrapped line should not be marked wrapped");
    }

    /// Shrink pushes top rows into scrollback to keep cursor visible,
    /// anchoring content to the bottom (matches macOS Terminal).
    #[test]

    /// Shrink + grow round-trip restores content from scrollback.
    #[test]

    #[test]

    #[test]
    fn makepad_tui_hi_up_down_resize_restores_framebuffer() {
        let mut t = setup_makepad_tui_like_with_hi(80, 20);
        render_like_tui_resize(&mut t, 20);
        let before = snapshot_grid(&t);
        let before_cursor = (t.screen().cursor.x, t.screen().cursor.y);

        for r in 21..=40 {
            t.resize(80, r);
            render_like_tui_resize(&mut t, r as u16);
        }
        for r in (20..40).rev() {
            t.resize(80, r);
            render_like_tui_resize(&mut t, r as u16);
        }

        let after = snapshot_grid(&t);
        let after_cursor = (t.screen().cursor.x, t.screen().cursor.y);
        if after != before {
            let first_diff = before
                .iter()
                .zip(after.iter())
                .enumerate()
                .find(|(_, (a, b))| a != b)
                .map(|(idx, _)| idx)
                .unwrap_or(0);
            let before_line: String = before[first_diff].iter().collect();
            let after_line: String = after[first_diff].iter().collect();
            panic!(
                "grid should round-trip after up/down resize; first differing row={} before='{}' after='{}'",
                first_diff,
                before_line.trim_end(),
                after_line.trim_end()
            );
        }
        assert_eq!(after_cursor, before_cursor, "cursor should round-trip after up/down resize");
    }

    #[test]
    fn codex_tui_up_down_resize_restores_framebuffer() {
        let mut t = setup_codex_tui(80, 20);
        render_like_codex_resize(&mut t, 80, 20);
        let before = snapshot_grid(&t);
        let before_cursor = (t.screen().cursor.x, t.screen().cursor.y);

        for r in 21..=40 {
            t.resize(80, r);
            render_like_codex_resize(&mut t, 80, r as u16);
        }
        for r in (20..40).rev() {
            t.resize(80, r);
            render_like_codex_resize(&mut t, 80, r as u16);
        }

        let after = snapshot_grid(&t);
        let after_cursor = (t.screen().cursor.x, t.screen().cursor.y);
        if after != before {
            let first_diff = before
                .iter()
                .zip(after.iter())
                .enumerate()
                .find(|(_, (a, b))| a != b)
                .map(|(idx, _)| idx)
                .unwrap_or(0);
            let before_line: String = before[first_diff].iter().collect();
            let after_line: String = after[first_diff].iter().collect();
            panic!(
                "codex grid should round-trip after up/down resize; first differing row={} before='{}' after='{}'",
                first_diff,
                before_line.trim_end(),
                after_line.trim_end()
            );
        }
        assert_eq!(
            after_cursor, before_cursor,
            "codex cursor should round-trip after up/down resize"
        );
    }

    #[test]
    fn codex_tui_stress_resize_redraw_matches_fresh_frame() {
        let mut t = setup_codex_tui(80, 20);
        render_like_codex_resize(&mut t, 80, 20);
        assert_codex_frame_invariants(&t, 80, 20);

        let sequence: &[(u16, u16)] = &[
            (100, 34),
            (72, 18),
            (120, 40),
            (64, 16),
            (90, 28),
            (80, 20),
        ];

        for _cycle in 0..3 {
            for (cols, rows) in sequence {
                t.resize(*cols as usize, *rows as usize);
                render_like_codex_resize(&mut t, *cols, *rows);
                assert_codex_frame_invariants(&t, *cols, *rows);
            }
        }

        // Final 80x20 redraw should match a fresh terminal at 80x20 exactly.
        t.resize(80, 20);
        render_like_codex_resize(&mut t, 80, 20);
        let final_snapshot = snapshot_grid(&t);

        let mut fresh = setup_codex_tui(80, 20);
        render_like_codex_resize(&mut fresh, 80, 20);
        let fresh_snapshot = snapshot_grid(&fresh);

        assert_eq!(
            final_snapshot, fresh_snapshot,
            "stress resize/redraw should converge to fresh codex frame"
        );
    }
}
