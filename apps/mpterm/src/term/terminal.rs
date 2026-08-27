//! The terminal: control-function semantics over two screens.
//!
//! Port of ghostty `src/terminal/Terminal.zig` + the CSI/ESC dispatch from
//! `stream_terminal.zig`, on the row-based Screen in this crate. Kitty
//! graphics, sixel, tmux control mode and the status line are not ported.

use crate::term::charsets::{Charset, Slot};
use crate::term::color::{default_palette, encode_color_reply, Palette, Rgb};
use crate::term::modes::{mode_from_int, Mode, ModeState};
use crate::term::osc::{ColorKind, ColorOp, OscCommand, OscTerminator};
use crate::term::page::{Cell, CellContent, Cluster, SemanticPrompt};
use crate::term::parser::{Action, Csi, Dcs, Esc};
use crate::term::screen::{CursorStyle, SavedCursor, Screen};
use crate::term::sgr::{attributes, Attribute};
use crate::term::style::{StyleColor, StyleFlags};
use crate::term::unicode::{char_width, grapheme_break, GraphemeState};

pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// Host-facing events that are not PTY reply bytes.
#[derive(Clone, Debug, PartialEq)]
pub enum TermEvent {
    TitleChanged(String),
    Bell,
    ClipboardSet { kind: u8, text: String },
    PwdChanged(String),
    Notification { title: String, body: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveScreen {
    Primary,
    Alternate,
}

pub struct Terminal {
    pub primary: Screen,
    pub alternate: Screen,
    pub active: ActiveScreen,

    pub modes: ModeState,
    pub tabstops: crate::term::tabstops::Tabstops,
    pub palette: Palette,
    pub default_fg: Rgb,
    pub default_bg: Rgb,
    pub cursor_color: Option<Rgb>,
    pub cursor_style: CursorStyle,
    pub title: String,
    pub pwd: String,

    /// Theme baseline restored on RIS / OSC 104/110/111/112.
    pub base_palette: Palette,
    pub base_fg: Rgb,
    pub base_bg: Rgb,

    /// Kitty keyboard flag stacks, one per screen (the spec keeps them
    /// separate so alt-screen apps can't corrupt the shell's state).
    pub kitty_flags_primary: Vec<u8>,
    pub kitty_flags_alternate: Vec<u8>,
    /// xterm modifyOtherKeys state (CSI > 4 ; m), 0/1/2.
    pub modify_other_keys: u8,

    /// Hyperlink table; cells store 1-based ids into this.
    pub hyperlinks: Vec<String>,
    current_hyperlink: u32,

    /// Last printed codepoint, for REP.
    previous_printed: Option<u32>,

    /// Reply bytes to write back to the PTY (DSR/DA/OSC queries...).
    pub outbound: Vec<u8>,
    /// Host-facing events since last take.
    pub events: Vec<TermEvent>,
    /// Any visible state changed since last take (render dirty).
    pub dirty: bool,

    // DCS accumulation (DECRQSS / XTGETTCAP replies only).
    dcs_kind: DcsKind,
    dcs_buf: Vec<u8>,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let palette = default_palette();
        let fg = Rgb::new(0xa9, 0xb1, 0xd6);
        let bg = Rgb::new(0x1a, 0x1b, 0x26);
        Self {
            primary: Screen::new(cols, rows, DEFAULT_SCROLLBACK),
            alternate: Screen::new(cols, rows, 0),
            active: ActiveScreen::Primary,
            modes: ModeState::new(),
            tabstops: crate::term::tabstops::Tabstops::new(cols),
            palette,
            default_fg: fg,
            default_bg: bg,
            cursor_color: None,
            cursor_style: CursorStyle::Default,
            title: String::new(),
            pwd: String::new(),
            base_palette: palette,
            base_fg: fg,
            base_bg: bg,
            kitty_flags_primary: Vec::new(),
            kitty_flags_alternate: Vec::new(),
            modify_other_keys: 0,
            hyperlinks: Vec::new(),
            current_hyperlink: 0,
            previous_printed: None,
            outbound: Vec::new(),
            events: Vec::new(),
            dirty: true,
            dcs_kind: DcsKind::Ignore,
            dcs_buf: Vec::new(),
        }
    }

    /// Install the app theme as the reset baseline (and current values).
    pub fn set_theme(&mut self, base16: &[Rgb; 16], fg: Rgb, bg: Rgb) {
        self.base_palette = default_palette();
        self.base_palette[..16].copy_from_slice(base16);
        self.base_fg = fg;
        self.base_bg = bg;
        self.palette = self.base_palette;
        self.default_fg = fg;
        self.default_bg = bg;
        self.dirty = true;
    }

    pub fn screen(&self) -> &Screen {
        match self.active {
            ActiveScreen::Primary => &self.primary,
            ActiveScreen::Alternate => &self.alternate,
        }
    }

    pub fn screen_mut(&mut self) -> &mut Screen {
        match self.active {
            ActiveScreen::Primary => &mut self.primary,
            ActiveScreen::Alternate => &mut self.alternate,
        }
    }

    pub fn cols(&self) -> usize {
        self.screen().cols
    }

    pub fn rows(&self) -> usize {
        self.screen().rows
    }

    pub fn take_outbound(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.outbound)
    }

    pub fn take_events(&mut self) -> Vec<TermEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn kitty_flags(&self) -> u8 {
        let stack = match self.active {
            ActiveScreen::Primary => &self.kitty_flags_primary,
            ActiveScreen::Alternate => &self.kitty_flags_alternate,
        };
        stack.last().copied().unwrap_or(0)
    }

    fn kitty_stack_mut(&mut self) -> &mut Vec<u8> {
        match self.active {
            ActiveScreen::Primary => &mut self.kitty_flags_primary,
            ActiveScreen::Alternate => &mut self.kitty_flags_alternate,
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols() && rows == self.rows() {
            return;
        }
        self.primary.resize_reflow(cols, rows);
        self.alternate.resize_clamped(cols, rows);
        self.tabstops.resize(cols);
        self.dirty = true;
        if self.modes.get(Mode::InBandSizeReports) {
            self.report_in_band_size();
        }
    }

    fn report_in_band_size(&mut self) {
        // Mode 2048 in-band size report: CSI 48 ; rows ; cols ; height ; width t
        // (pixel sizes unknown here; 0 is allowed).
        let s = format!("\x1b[48;{};{};0;0t", self.rows(), self.cols());
        self.outbound.extend_from_slice(s.as_bytes());
    }

    // ==================================================================
    // Dispatch
    // ==================================================================

    pub fn dispatch(&mut self, action: Action) {
        match action {
            Action::Print(c) => self.print(c as u32),
            Action::Execute(b) => self.execute(b),
            Action::CsiDispatch(csi) => self.csi_dispatch(&csi),
            Action::EscDispatch(esc) => self.esc_dispatch(&esc),
            Action::OscDispatch(cmd) => self.osc_dispatch(cmd),
            Action::DcsHook(dcs) => self.dcs_hook(&dcs),
            Action::DcsPut(b) => self.dcs_put(b),
            Action::DcsUnhook => self.dcs_unhook(),
            Action::ApcStart | Action::ApcPut(_) | Action::ApcEnd => {}
        }
    }

    // ==================================================================
    // Printing
    // ==================================================================

    /// The exclusive right limit for printing at the current cursor
    /// (ghostty: outside the right margin the full width applies).
    fn right_limit(&self) -> usize {
        let s = self.screen();
        if s.cursor.x > s.right_margin {
            s.cols
        } else {
            s.right_margin + 1
        }
    }

    pub fn print(&mut self, cp: u32) {
        self.dirty = true;
        let right_limit = self.right_limit();

        // Grapheme clustering (mode 2027): join with the previous cell when
        // UAX #29 says there is no boundary.
        if cp > 255 && self.modes.get(Mode::GraphemeCluster) && self.screen().cursor.x > 0 {
            if self.try_grapheme_attach(cp, right_limit) {
                return;
            }
        }

        let width = if cp <= 0xff { 1 } else { char_width(cp) };

        if width == 0 {
            // Zero-width: attach to the previous cell as combining data
            // (without 2027; with 2027 the cluster path above handles it).
            if self.modes.get(Mode::GraphemeCluster) {
                return;
            }
            self.attach_zero_width(cp);
            return;
        }

        self.previous_printed = Some(cp);

        if self.screen().cursor.pending_wrap && self.modes.get(Mode::Wraparound) {
            self.print_wrap();
        }

        // Insert mode shifts the line right first.
        if self.modes.get(Mode::Insert) && self.screen().cursor.x + width as usize <= self.cols()
        {
            self.insert_blanks(width as usize);
        }

        let right_limit = self.right_limit();
        match width {
            1 => self.print_cell(cp, false),
            _ => {
                let left = self.screen().left_margin;
                if right_limit.saturating_sub(left) > 1 {
                    if self.screen().cursor.x == right_limit - 1 {
                        // Wide char at the margin: spacer head (only at the
                        // true screen edge) then wrap.
                        if !self.modes.get(Mode::Wraparound) {
                            return;
                        }
                        if right_limit == self.cols() {
                            let y = self.screen().cursor.y;
                            self.screen_mut().row_mut(y).wrapped = true;
                            self.put_content(CellContent::WideSpacerHead);
                        } else {
                            self.put_content(CellContent::Empty);
                        }
                        self.print_wrap();
                    }
                    self.print_cell(cp, true);
                    let s = self.screen_mut();
                    s.cursor.x += 1;
                    self.put_content(CellContent::WideTail);
                } else {
                    // A 1-wide region can't hold a wide char.
                    self.put_content(CellContent::Empty);
                }
            }
        }

        let s = self.screen_mut();
        let right_limit = if s.cursor.x > s.right_margin {
            s.cols
        } else {
            s.right_margin + 1
        };
        if s.cursor.x == right_limit - 1 {
            s.cursor.pending_wrap = true;
        } else {
            s.cursor.x += 1;
        }
    }

    /// Mode-2027 path: returns true when `cp` was absorbed into the
    /// previous cell's cluster (possibly with width adjustments).
    fn try_grapheme_attach(&mut self, cp: u32, right_limit: usize) -> bool {
        let s = self.screen();
        let wraparound = self.modes.get(Mode::Wraparound);
        // Which cell is "previous": under the cursor while a wrap is
        // pending, otherwise one to the left.
        let left = if wraparound {
            if s.cursor.pending_wrap {
                0
            } else {
                1
            }
        } else if s.cursor.x != right_limit - 1 {
            1
        } else {
            let cur_empty = s
                .row(s.cursor.y)
                .cell(s.cursor.x)
                .map(|c| c.content.primary().is_none())
                .unwrap_or(true);
            if cur_empty {
                1
            } else {
                0
            }
        };
        if s.cursor.x < left {
            return false;
        }
        let mut px = s.cursor.x - left;
        let py = s.cursor.y;
        // Step over a wide tail to its head.
        px = s.row(py).head_of(px);

        let prev_content = match s.row(py).cell(px) {
            Some(c) => c.content.clone(),
            None => return false,
        };
        let cps: Vec<char> = match &prev_content {
            CellContent::Char(c) | CellContent::WideChar(c) => vec![*c],
            CellContent::Cluster(cl) => cl.cps.clone(),
            _ => return false,
        };

        // Replay the cluster through the break state, then test cp.
        let mut state = GraphemeState::default();
        let mut prev_cp = cps[0] as u32;
        for &c in &cps[1..] {
            let _ = grapheme_break(prev_cp, c as u32, &mut state);
            prev_cp = c as u32;
        }
        if grapheme_break(prev_cp, cp, &mut state) {
            return false;
        }

        // No boundary: append. Emoji variation selectors can change width.
        let old_width = prev_content.width();
        let mut new_width = old_width;
        if cp == 0xfe0f && old_width == 1 {
            new_width = 2;
        } else if cp == 0xfe0e && old_width == 2 {
            new_width = 1;
        }

        let ch = match char::from_u32(cp) {
            Some(ch) => ch,
            None => return true,
        };
        let mut new_cps = cps;
        new_cps.push(ch);
        let cluster = CellContent::Cluster(Box::new(Cluster {
            cps: new_cps,
            width: new_width,
        }));

        if new_width == old_width {
            let sm = self.screen_mut();
            sm.row_mut(py).cell_mut(px).content = cluster;
            return true;
        }

        // Width changed. Narrow->wide needs room for a tail; wide->narrow
        // frees its tail.
        if new_width == 2 {
            let cols = self.cols();
            if px + 1 >= right_limit.min(cols) {
                // No room: wrap the (now wide) cluster to the next line.
                if !wraparound {
                    return true;
                }
                let pen = self.screen().cursor.style;
                {
                    let sm = self.screen_mut();
                    sm.cursor.x = px;
                    sm.cursor.y = py;
                    let row = sm.row_mut(py);
                    *row.cell_mut(px) = Cell {
                        content: if right_limit == cols {
                            CellContent::WideSpacerHead
                        } else {
                            CellContent::Empty
                        },
                        style: crate::term::style::Style {
                            bg_color: pen.bg_color,
                            ..Default::default()
                        },
                        hyperlink: 0,
                    };
                    if right_limit == cols {
                        row.wrapped = true;
                    }
                }
                self.print_wrap();
                let sm = self.screen_mut();
                let (x, y) = (sm.cursor.x, sm.cursor.y);
                sm.row_mut(y).cell_mut(x).content = cluster;
                sm.cursor.x = x + 1;
                let ry = sm.cursor.y;
                let rx = sm.cursor.x;
                sm.row_mut(ry).cell_mut(rx).content = CellContent::WideTail;
                let rl = if sm.cursor.x > sm.right_margin {
                    sm.cols
                } else {
                    sm.right_margin + 1
                };
                if sm.cursor.x == rl - 1 {
                    sm.cursor.pending_wrap = true;
                } else {
                    sm.cursor.x += 1;
                }
            } else {
                let sm = self.screen_mut();
                sm.row_mut(py).cell_mut(px).content = cluster;
                let tail = sm.row_mut(py).cell_mut(px + 1);
                tail.content = CellContent::WideTail;
                // The cursor sits after the tail now.
                sm.cursor.x = (px + 2).min(sm.cols - 1);
                sm.cursor.pending_wrap = px + 2 >= right_limit;
            }
        } else {
            // Wide -> narrow: drop the tail.
            let sm = self.screen_mut();
            sm.row_mut(py).cell_mut(px).content = cluster;
            if px + 1 < sm.cols {
                let tail = sm.row_mut(py).cell_mut(px + 1);
                if tail.content == CellContent::WideTail {
                    tail.content = CellContent::Empty;
                }
            }
            sm.cursor.pending_wrap = false;
            sm.cursor.x = (px + 1).min(right_limit - 1);
        }
        true
    }

    /// Attach a zero-width codepoint to the previous cell (non-2027 path).
    fn attach_zero_width(&mut self, cp: u32) {
        let s = self.screen();
        let left = if self.modes.get(Mode::Wraparound) && s.cursor.pending_wrap {
            0
        } else {
            1
        };
        if s.cursor.x == 0 && left == 1 {
            return;
        }
        let mut px = s.cursor.x - left;
        let py = s.cursor.y;
        px = s.row(py).head_of(px);
        let ch = match char::from_u32(cp) {
            Some(ch) => ch,
            None => return,
        };
        let sm = self.screen_mut();
        let cell = sm.row_mut(py).cell_mut(px);
        match &mut cell.content {
            CellContent::Char(c) => {
                cell.content = CellContent::Cluster(Box::new(Cluster {
                    cps: vec![*c, ch],
                    width: 1,
                }));
            }
            CellContent::WideChar(c) => {
                cell.content = CellContent::Cluster(Box::new(Cluster {
                    cps: vec![*c, ch],
                    width: 2,
                }));
            }
            CellContent::Cluster(cl) => cl.cps.push(ch),
            _ => {}
        }
    }

    /// Soft wrap: mark the row wrapped (at the true edge), index, return to
    /// the left margin, extend prompt semantics.
    fn print_wrap(&mut self) {
        let at_edge = self.screen().cursor.x == self.cols() - 1;
        let y = self.screen().cursor.y;
        if at_edge {
            self.screen_mut().row_mut(y).wrapped = true;
        }
        let old_semantic = self.screen().row(y).semantic;
        self.index();
        let s = self.screen_mut();
        s.cursor.x = s.left_margin;
        s.cursor.pending_wrap = false;
        let ny = s.cursor.y;
        if old_semantic == SemanticPrompt::Prompt
            || old_semantic == SemanticPrompt::PromptContinuation
        {
            s.row_mut(ny).semantic = SemanticPrompt::PromptContinuation;
        }
    }

    /// Write content at the cursor without moving it, splitting any wide
    /// neighbors and applying pen style + hyperlink.
    fn put_content(&mut self, content: CellContent) {
        let pen = self.screen().cursor.style;
        let link = self.current_hyperlink;
        let sm = self.screen_mut();
        let (x, y) = (sm.cursor.x, sm.cursor.y);
        let row = sm.row_mut(y);
        row.split_wide_at(x, &pen);
        let cell = row.cell_mut(x);
        cell.content = content;
        cell.style = pen;
        cell.hyperlink = link;
    }

    fn print_cell(&mut self, cp: u32, wide: bool) {
        // Charset mapping applies to GL range.
        let cp = if cp <= 0x7f {
            self.screen_mut().charsets.map_gl(cp)
        } else {
            // A pending single shift is consumed even by non-GL input.
            self.screen_mut().charsets.single_shift = None;
            cp
        };
        let Some(ch) = char::from_u32(cp) else { return };
        let content = if wide {
            CellContent::WideChar(ch)
        } else {
            CellContent::Char(ch)
        };
        self.put_content(content);
    }

    // ==================================================================
    // C0 controls
    // ==================================================================

    pub fn execute(&mut self, byte: u8) {
        match byte {
            0x05 => {} // ENQ: no answerback
            0x07 => self.events.push(TermEvent::Bell),
            0x08 => self.cursor_left(1),
            0x09 => self.horizontal_tab(),
            0x0a | 0x0b | 0x0c => {
                self.index();
                if self.modes.get(Mode::Linefeed) {
                    self.carriage_return();
                }
                self.dirty = true;
            }
            0x0d => {
                self.carriage_return();
                self.dirty = true;
            }
            0x0e => self.screen_mut().charsets.gl = Slot::G1, // SO
            0x0f => self.screen_mut().charsets.gl = Slot::G0, // SI
            _ => {}
        }
    }

    fn carriage_return(&mut self) {
        let s = self.screen_mut();
        s.cursor.x = if s.cursor.x >= s.left_margin {
            s.left_margin
        } else {
            0
        };
        s.cursor.pending_wrap = false;
    }

    fn horizontal_tab(&mut self) {
        let right_limit = self.right_limit();
        let s = self.screen();
        let next = self.tabstops.next_after(s.cursor.x).min(right_limit - 1);
        let s = self.screen_mut();
        s.cursor.x = next.max(s.cursor.x);
        s.cursor.pending_wrap = false;
    }

    fn horizontal_tab_back(&mut self, count: usize) {
        let s = self.screen();
        let mut x = s.cursor.x;
        for _ in 0..count.max(1) {
            x = self.tabstops.prev_before(x);
            if x == 0 {
                break;
            }
        }
        let left = self.screen().left_margin;
        let s = self.screen_mut();
        s.cursor.x = x.max(if s.cursor.x >= left { left } else { 0 });
        s.cursor.pending_wrap = false;
    }

    /// IND: move down, scrolling at the bottom of the scroll region (with
    /// scrollback when the region is the whole screen).
    pub fn index(&mut self) {
        self.dirty = true;
        let s = self.screen();
        let (top, bottom) = (s.scroll_top, s.scroll_bottom);
        let (left, right) = (s.left_margin, s.right_margin);
        let (x, y) = (s.cursor.x, s.cursor.y);
        let rows = s.rows;
        self.screen_mut().cursor.pending_wrap = false;

        if y < top || y > bottom {
            if y < rows - 1 {
                self.screen_mut().cursor.y += 1;
            }
            return;
        }

        if y == bottom && x >= left && x <= right {
            let pen = self.screen().cursor.style;
            self.screen_mut().scroll_up(1, &pen);
            return;
        }

        if y < bottom {
            self.screen_mut().cursor.y += 1;
        }
    }

    /// RI: move up, scrolling down at the top of the scroll region.
    fn reverse_index(&mut self) {
        self.dirty = true;
        let s = self.screen();
        if s.cursor.y != s.scroll_top || s.cursor.x < s.left_margin || s.cursor.x > s.right_margin
        {
            self.cursor_up(1);
        } else {
            let pen = self.screen().cursor.style;
            self.screen_mut().scroll_down(1, &pen);
        }
    }

    // ==================================================================
    // Cursor movement
    // ==================================================================

    fn cursor_up(&mut self, count: usize) {
        let count = count.max(1);
        let s = self.screen_mut();
        // Clamp to the scroll region top only if we start below it.
        let min_y = if s.cursor.y >= s.scroll_top {
            s.scroll_top
        } else {
            0
        };
        s.cursor.y = s.cursor.y.saturating_sub(count).max(min_y);
        s.cursor.pending_wrap = false;
    }

    fn cursor_down(&mut self, count: usize) {
        let count = count.max(1);
        let s = self.screen_mut();
        let max_y = if s.cursor.y <= s.scroll_bottom {
            s.scroll_bottom
        } else {
            s.rows - 1
        };
        s.cursor.y = (s.cursor.y + count).min(max_y);
        s.cursor.pending_wrap = false;
    }

    fn cursor_right(&mut self, count: usize) {
        let count = count.max(1);
        let s = self.screen_mut();
        let max_x = if s.cursor.x <= s.right_margin {
            s.right_margin
        } else {
            s.cols - 1
        };
        s.cursor.x = (s.cursor.x + count).min(max_x);
        s.cursor.pending_wrap = false;
    }

    /// CUB with reverse-wrap modes (ghostty cursorLeft).
    fn cursor_left(&mut self, count: usize) {
        #[derive(PartialEq)]
        enum WrapMode {
            None,
            Reverse,
            ReverseExtended,
        }
        let wrap_mode = if !self.modes.get(Mode::Wraparound) {
            WrapMode::None
        } else if self.modes.get(Mode::ReverseWrapExtended) {
            WrapMode::ReverseExtended
        } else if self.modes.get(Mode::ReverseWrap) {
            WrapMode::Reverse
        } else {
            WrapMode::None
        };

        let mut count = count.max(1);

        if wrap_mode == WrapMode::None {
            let s = self.screen_mut();
            s.cursor.x = s.cursor.x.saturating_sub(count);
            s.cursor.pending_wrap = false;
            return;
        }

        if self.screen().cursor.pending_wrap {
            count -= 1;
            self.screen_mut().cursor.pending_wrap = false;
        }

        let (top, bottom, right_margin) = {
            let s = self.screen();
            (s.scroll_top, s.scroll_bottom, s.right_margin)
        };
        let left_margin = {
            let s = self.screen();
            if s.cursor.x < s.left_margin {
                0
            } else {
                s.left_margin
            }
        };

        if self.screen().cursor.x == left_margin
            && wrap_mode == WrapMode::Reverse
            && self.screen().cursor.y <= top
        {
            let s = self.screen_mut();
            s.cursor.x = left_margin;
            s.cursor.y = top;
            return;
        }

        loop {
            let s = self.screen_mut();
            let max = s.cursor.x - left_margin;
            let amount = max.min(count);
            count -= amount;
            s.cursor.x -= amount;
            if count == 0 {
                break;
            }
            if s.cursor.y == top {
                if wrap_mode != WrapMode::ReverseExtended {
                    break;
                }
                s.cursor.x = right_margin;
                s.cursor.y = bottom;
                count -= 1;
                continue;
            }
            if s.cursor.y == 0 {
                break;
            }
            if wrap_mode != WrapMode::ReverseExtended {
                let prev_wrapped = s.row(s.cursor.y - 1).wrapped;
                if !prev_wrapped {
                    break;
                }
            }
            s.cursor.x = right_margin;
            s.cursor.y -= 1;
            count -= 1;
        }
    }

    /// CUP/HVP, honoring origin mode.
    fn set_cursor_pos(&mut self, row_req: usize, col_req: usize) {
        let origin = self.modes.get(Mode::Origin);
        let s = self.screen_mut();
        let (x_off, y_off, x_max, y_max) = if origin {
            (
                s.left_margin,
                s.scroll_top,
                s.right_margin + 1,
                s.scroll_bottom + 1,
            )
        } else {
            (0, 0, s.cols, s.rows)
        };
        s.cursor.pending_wrap = false;
        let row = row_req.max(1);
        let col = col_req.max(1);
        s.cursor.x = (col + x_off).min(x_max) - 1;
        s.cursor.y = (row + y_off).min(y_max) - 1;
    }

    // ==================================================================
    // Save/restore cursor
    // ==================================================================

    fn save_cursor(&mut self) {
        let s = self.screen();
        let saved = SavedCursor {
            x: s.cursor.x,
            y: s.cursor.y,
            style: s.cursor.style,
            pending_wrap: s.cursor.pending_wrap,
            origin: self.modes.get(Mode::Origin),
            charsets: s.charsets.clone(),
        };
        self.screen_mut().saved_cursor = Some(saved);
    }

    fn restore_cursor(&mut self) {
        let Some(saved) = self.screen().saved_cursor.clone() else {
            // No save: restore to defaults (xterm behavior).
            self.modes.set(Mode::Origin, false);
            let s = self.screen_mut();
            s.cursor.x = 0;
            s.cursor.y = 0;
            s.cursor.pending_wrap = false;
            s.cursor.style = Default::default();
            s.charsets = Default::default();
            return;
        };
        self.modes.set(Mode::Origin, saved.origin);
        let s = self.screen_mut();
        s.cursor.x = saved.x.min(s.cols - 1);
        s.cursor.y = saved.y.min(s.rows - 1);
        s.cursor.style = saved.style;
        s.cursor.pending_wrap = saved.pending_wrap;
        s.charsets = saved.charsets;
        self.dirty = true;
    }

    // ==================================================================
    // Editing
    // ==================================================================

    /// IL: insert blank lines at the cursor within the scroll region.
    fn insert_lines(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let s = self.screen();
        let (y, x) = (s.cursor.y, s.cursor.x);
        if y < s.scroll_top || y > s.scroll_bottom || x < s.left_margin || x > s.right_margin {
            return;
        }
        self.dirty = true;
        let pen = self.screen().cursor.style;
        let s = self.screen_mut();
        let old_top = s.scroll_top;
        s.scroll_top = y;
        s.scroll_down(count, &pen);
        s.scroll_top = old_top;
        s.cursor.x = s.left_margin;
        s.cursor.pending_wrap = false;
    }

    /// DL: delete lines at the cursor within the scroll region.
    fn delete_lines(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let s = self.screen();
        let (y, x) = (s.cursor.y, s.cursor.x);
        if y < s.scroll_top || y > s.scroll_bottom || x < s.left_margin || x > s.right_margin {
            return;
        }
        self.dirty = true;
        let pen = self.screen().cursor.style;
        let s = self.screen_mut();
        let old_top = s.scroll_top;
        let old_max = s.max_scrollback;
        s.scroll_top = y;
        s.max_scrollback = 0; // region delete never feeds history
        let save_len = s.scrollback.len();
        s.scroll_up(count, &pen);
        debug_assert_eq!(save_len, s.scrollback.len());
        s.max_scrollback = old_max;
        s.scroll_top = old_top;
        s.cursor.x = s.left_margin;
        s.cursor.pending_wrap = false;
    }

    /// ICH: insert blanks at the cursor, shifting right within margins.
    fn insert_blanks(&mut self, count: usize) {
        let s = self.screen();
        if s.cursor.x < s.left_margin || s.cursor.x > s.right_margin {
            return;
        }
        self.dirty = true;
        let pen = self.screen().cursor.style;
        let s = self.screen_mut();
        let (x, y, right) = (s.cursor.x, s.cursor.y, s.right_margin);
        s.cursor.pending_wrap = false;
        let count = count.max(1).min(right + 1 - x);
        let row = s.row_mut(y);
        row.split_wide_at(x, &pen);
        // Make sure storage covers the margin span, then shift.
        row.cell_mut(right);
        for col in ((x + count)..=right).rev() {
            row.cells[col] = row.cells[col - count].clone();
        }
        let blank = Cell::blank_with_bg(&pen);
        for col in x..x + count {
            row.cells[col] = blank.clone();
        }
        // A wide char split across the right margin can't survive.
        row.split_wide_at(right, &pen);
        if row
            .cell(right)
            .map(|c| c.content.is_wide_head())
            .unwrap_or(false)
        {
            row.cells[right] = blank;
        }
    }

    /// DCH: delete chars at the cursor, shifting left within margins.
    fn delete_chars(&mut self, count: usize) {
        let s = self.screen();
        if s.cursor.x < s.left_margin || s.cursor.x > s.right_margin {
            return;
        }
        self.dirty = true;
        let pen = self.screen().cursor.style;
        let s = self.screen_mut();
        let (x, y, right) = (s.cursor.x, s.cursor.y, s.right_margin);
        s.cursor.pending_wrap = false;
        let count = count.max(1).min(right + 1 - x);
        let row = s.row_mut(y);
        row.split_wide_at(x, &pen);
        row.cell_mut(right);
        for col in x..=right {
            if col + count <= right {
                row.cells[col] = row.cells[col + count].clone();
            } else {
                row.cells[col] = Cell::blank_with_bg(&pen);
            }
        }
        // Don't leave a dangling tail at the start of the shifted span.
        if row.cell(x).map(|c| c.content == CellContent::WideTail).unwrap_or(false) {
            row.cells[x] = Cell::blank_with_bg(&pen);
        }
    }

    /// ECH: erase chars from the cursor (no shifting).
    fn erase_chars(&mut self, count: usize) {
        self.dirty = true;
        let pen = self.screen().cursor.style;
        let s = self.screen_mut();
        let (x, y) = (s.cursor.x, s.cursor.y);
        let count = count.max(1).min(s.cols - x);
        s.cursor.pending_wrap = false;
        let row = s.row_mut(y);
        row.split_wide_at(x, &pen);
        row.split_wide_at(x + count - 1, &pen);
        let blank = Cell::blank_with_bg(&pen);
        for col in x..x + count {
            *row.cell_mut(col) = blank.clone();
        }
    }

    /// EL.
    fn erase_line(&mut self, mode: u16) {
        self.dirty = true;
        let pen = self.screen().cursor.style;
        let s = self.screen_mut();
        let (mut x, y) = (s.cursor.x, s.cursor.y);
        let cols = s.cols;
        let (start, end) = match mode {
            0 => {
                // Right: include a wide head whose tail we'd otherwise cut.
                x = s.row(y).head_of(x);
                s.row_mut(y).wrapped = false;
                (x, cols)
            }
            1 => {
                // Left: include the tail of a wide char under the cursor.
                let extra = s
                    .row(y)
                    .cell(x)
                    .map(|c| c.content.is_wide_head())
                    .unwrap_or(false);
                (0, x + 1 + extra as usize)
            }
            2 => {
                s.row_mut(y).wrapped = false;
                (0, cols)
            }
            _ => return,
        };
        s.cursor.pending_wrap = false;
        let blank = Cell::blank_with_bg(&pen);
        let row = s.row_mut(y);
        if start == 0 && end == cols && blank.is_default() {
            row.cells.clear();
            return;
        }
        for col in start..end.min(cols) {
            *row.cell_mut(col) = blank.clone();
        }
        row.trim();
    }

    /// ED.
    fn erase_display(&mut self, mode: u16) {
        self.dirty = true;
        let pen = self.screen().cursor.style;
        match mode {
            0 => {
                // Below: EL right + clear rows below.
                self.erase_line(0);
                let s = self.screen_mut();
                let y = s.cursor.y;
                let cols = s.cols;
                for row_y in (y + 1)..s.rows {
                    s.row_mut(row_y).clear(&pen, cols);
                }
            }
            1 => {
                // Above: rows above + EL left.
                let s = self.screen_mut();
                let y = s.cursor.y;
                let cols = s.cols;
                for row_y in 0..y {
                    s.row_mut(row_y).clear(&pen, cols);
                }
                self.erase_line(1);
                self.screen_mut().cursor.pending_wrap = false;
            }
            2 => {
                let s = self.screen_mut();
                let cols = s.cols;
                for y in 0..s.rows {
                    s.row_mut(y).clear(&pen, cols);
                }
                s.cursor.pending_wrap = false;
            }
            3 => {
                let s = self.screen_mut();
                let evicted = s.scrollback.len() as u64;
                s.scrollback.clear();
                s.evicted += evicted;
            }
            22 => {
                // Kitty scroll-and-clear: push the screen contents into
                // scrollback, then clear.
                let s = self.screen_mut();
                let used = s.last_used_row() + 1;
                let pen_copy = pen;
                s.scroll_top = 0;
                s.scroll_bottom = s.rows - 1;
                for _ in 0..used {
                    s.scroll_up(1, &pen_copy);
                }
                s.cursor.pending_wrap = false;
            }
            _ => {}
        }
    }

    /// DECALN: fill with 'E', reset margins, home.
    fn decaln(&mut self) {
        self.dirty = true;
        let s = self.screen_mut();
        s.reset_margins();
        s.cursor.x = 0;
        s.cursor.y = 0;
        s.cursor.pending_wrap = false;
        let cols = s.cols;
        for y in 0..s.rows {
            let row = s.row_mut(y);
            row.wrapped = false;
            row.cells.clear();
            for x in 0..cols {
                *row.cell_mut(x) = Cell {
                    content: CellContent::Char('E'),
                    ..Default::default()
                };
            }
        }
    }

    /// REP: repeat the last printed codepoint.
    fn repeat_previous(&mut self, count: usize) {
        let Some(cp) = self.previous_printed else {
            return;
        };
        // Cap like ghostty (avoid a hostile REP blowing us up).
        let count = count.max(1).min(self.cols() * self.rows());
        for _ in 0..count {
            self.print(cp);
        }
    }

    // ==================================================================
    // Scrolling (CSI S/T)
    // ==================================================================

    fn scroll_up_cmd(&mut self, count: usize) {
        self.dirty = true;
        let pen = self.screen().cursor.style;
        let s = self.screen_mut();
        let (x, y) = (s.cursor.x, s.cursor.y);
        s.scroll_up(count.max(1), &pen);
        s.cursor.x = x;
        s.cursor.y = y;
        s.cursor.pending_wrap = false;
    }

    fn scroll_down_cmd(&mut self, count: usize) {
        self.dirty = true;
        let pen = self.screen().cursor.style;
        let s = self.screen_mut();
        let (x, y) = (s.cursor.x, s.cursor.y);
        s.scroll_down(count.max(1), &pen);
        s.cursor.x = x;
        s.cursor.y = y;
        s.cursor.pending_wrap = false;
    }

    // ==================================================================
    // SGR
    // ==================================================================

    fn apply_sgr(&mut self, csi: &Csi) {
        let attrs: Vec<Attribute> = attributes(csi).collect();
        let style = &mut self.screen_mut().cursor.style;
        for attr in attrs {
            match attr {
                Attribute::Unset => *style = Default::default(),
                Attribute::Bold => style.flags.set(StyleFlags::BOLD, true),
                Attribute::ResetBold => {
                    style.flags.set(StyleFlags::BOLD, false);
                    style.flags.set(StyleFlags::FAINT, false);
                }
                Attribute::Faint => style.flags.set(StyleFlags::FAINT, true),
                Attribute::Italic => style.flags.set(StyleFlags::ITALIC, true),
                Attribute::ResetItalic => style.flags.set(StyleFlags::ITALIC, false),
                Attribute::Underline(u) => style.flags.set_underline(u),
                Attribute::ResetUnderline => {
                    style.flags.set_underline(crate::term::style::Underline::None)
                }
                Attribute::UnderlineColorRgb(rgb) => {
                    style.underline_color = StyleColor::Rgb(rgb)
                }
                Attribute::UnderlineColorPalette(i) => {
                    style.underline_color = StyleColor::Palette(i)
                }
                Attribute::ResetUnderlineColor => style.underline_color = StyleColor::None,
                Attribute::Overline => style.flags.set(StyleFlags::OVERLINE, true),
                Attribute::ResetOverline => style.flags.set(StyleFlags::OVERLINE, false),
                Attribute::Blink => style.flags.set(StyleFlags::BLINK, true),
                Attribute::ResetBlink => style.flags.set(StyleFlags::BLINK, false),
                Attribute::Inverse => style.flags.set(StyleFlags::INVERSE, true),
                Attribute::ResetInverse => style.flags.set(StyleFlags::INVERSE, false),
                Attribute::Invisible => style.flags.set(StyleFlags::INVISIBLE, true),
                Attribute::ResetInvisible => style.flags.set(StyleFlags::INVISIBLE, false),
                Attribute::Strikethrough => style.flags.set(StyleFlags::STRIKETHROUGH, true),
                Attribute::ResetStrikethrough => {
                    style.flags.set(StyleFlags::STRIKETHROUGH, false)
                }
                Attribute::Fg8(n) => style.fg_color = StyleColor::Palette(n),
                Attribute::FgBright(n) => style.fg_color = StyleColor::Palette(n),
                Attribute::ResetFg => style.fg_color = StyleColor::None,
                Attribute::Bg8(n) => style.bg_color = StyleColor::Palette(n),
                Attribute::BgBright(n) => style.bg_color = StyleColor::Palette(n),
                Attribute::ResetBg => style.bg_color = StyleColor::None,
                Attribute::Fg256(n) => style.fg_color = StyleColor::Palette(n),
                Attribute::Bg256(n) => style.bg_color = StyleColor::Palette(n),
                Attribute::FgRgb(rgb) => style.fg_color = StyleColor::Rgb(rgb),
                Attribute::BgRgb(rgb) => style.bg_color = StyleColor::Rgb(rgb),
                Attribute::Unknown(_) => {}
            }
        }
    }

    // ==================================================================
    // Modes
    // ==================================================================

    fn set_mode(&mut self, value: u16, ansi: bool, enable: bool) {
        // Screen-switch pseudo-modes first: they aren't plain flags.
        if !ansi {
            match value {
                47 => {
                    self.switch_screen(enable, false, false);
                    return;
                }
                1047 => {
                    // Clear the alt screen when leaving it.
                    self.switch_screen(enable, false, true);
                    return;
                }
                1048 => {
                    if enable {
                        self.save_cursor();
                    } else {
                        self.restore_cursor();
                    }
                    return;
                }
                1049 => {
                    self.switch_screen_1049(enable);
                    return;
                }
                _ => {}
            }
        }

        let Some(mode) = mode_from_int(value, ansi) else {
            return;
        };
        self.modes.set(mode, enable);
        self.dirty = true;

        match mode {
            Mode::Origin => {
                self.set_cursor_pos(1, 1);
            }
            Mode::EnableLeftAndRightMargin => {
                if !enable {
                    let s = self.screen_mut();
                    s.left_margin = 0;
                    s.right_margin = s.cols - 1;
                }
            }
            _ => {}
        }
    }

    fn switch_screen(&mut self, to_alt: bool, save_cursor: bool, clear_on_leave: bool) {
        self.dirty = true;
        if to_alt && self.active == ActiveScreen::Primary {
            if save_cursor {
                self.save_cursor();
            }
            // Cursor position and pen carry over (xterm shares the cursor).
            let cursor = self.primary.cursor.clone();
            self.active = ActiveScreen::Alternate;
            let s = self.screen_mut();
            s.cursor = cursor;
            s.cursor.x = s.cursor.x.min(s.cols - 1);
            s.cursor.y = s.cursor.y.min(s.rows - 1);
        } else if !to_alt && self.active == ActiveScreen::Alternate {
            if clear_on_leave {
                let pen = self.alternate.cursor.style;
                let cols = self.alternate.cols;
                for y in 0..self.alternate.rows {
                    self.alternate.row_mut(y).clear(&pen, cols);
                }
            }
            let cursor = self.alternate.cursor.clone();
            self.active = ActiveScreen::Primary;
            let s = self.screen_mut();
            s.cursor = cursor;
            s.cursor.x = s.cursor.x.min(s.cols - 1);
            s.cursor.y = s.cursor.y.min(s.rows - 1);
            if save_cursor {
                self.restore_cursor();
            }
        }
    }

    fn switch_screen_1049(&mut self, enable: bool) {
        self.dirty = true;
        if enable && self.active == ActiveScreen::Primary {
            self.save_cursor();
            self.active = ActiveScreen::Alternate;
            // Clear alt and home the cursor, keeping the pen.
            let pen = self.primary.cursor.style;
            let s = self.screen_mut();
            let cols = s.cols;
            for y in 0..s.rows {
                s.row_mut(y).clear(&Default::default(), cols);
            }
            s.cursor = Default::default();
            s.cursor.style = pen;
            s.reset_margins();
        } else if !enable && self.active == ActiveScreen::Alternate {
            self.active = ActiveScreen::Primary;
            self.restore_cursor();
        }
    }

    // ==================================================================
    // CSI dispatch
    // ==================================================================

    fn csi_dispatch(&mut self, csi: &Csi) {
        let private = csi.is_private();
        match (csi.final_byte, csi.intermediates()) {
            (b'A', []) => self.cursor_up(csi.get(0, 1) as usize),
            (b'B', []) | (b'e', []) => self.cursor_down(csi.get(0, 1) as usize),
            (b'C', []) | (b'a', []) => self.cursor_right(csi.get(0, 1) as usize),
            (b'D', []) => self.cursor_left(csi.get(0, 1) as usize),
            (b'E', []) => {
                self.cursor_down(csi.get(0, 1) as usize);
                self.carriage_return();
            }
            (b'F', []) => {
                self.cursor_up(csi.get(0, 1) as usize);
                self.carriage_return();
            }
            (b'G', []) | (b'`', []) => {
                // CHA/HPA: absolute column, 1-based, no margins/origin
                // except origin mode offsets by the left margin.
                let col = csi.get(0, 1) as usize;
                let origin = self.modes.get(Mode::Origin);
                let s = self.screen_mut();
                let (off, max) = if origin {
                    (s.left_margin, s.right_margin + 1)
                } else {
                    (0, s.cols)
                };
                s.cursor.x = (col + off).min(max) - 1;
                s.cursor.pending_wrap = false;
                self.dirty = true;
            }
            (b'H', []) | (b'f', []) => {
                self.set_cursor_pos(csi.get(0, 1) as usize, csi.get(1, 1) as usize);
                self.dirty = true;
            }
            (b'I', []) => {
                for _ in 0..csi.get(0, 1) {
                    self.horizontal_tab();
                }
            }
            (b'J', []) => self.erase_display(csi.get_allow_zero(0, 0)),
            (b'J', [b'?']) => self.erase_display(csi.get_allow_zero(0, 0)),
            (b'K', []) => self.erase_line(csi.get_allow_zero(0, 0)),
            (b'K', [b'?']) => self.erase_line(csi.get_allow_zero(0, 0)),
            (b'L', []) => self.insert_lines(csi.get(0, 1) as usize),
            (b'M', []) => self.delete_lines(csi.get(0, 1) as usize),
            (b'P', []) => self.delete_chars(csi.get(0, 1) as usize),
            (b'S', []) => self.scroll_up_cmd(csi.get(0, 1) as usize),
            (b'T', []) if csi.params_len <= 1 => self.scroll_down_cmd(csi.get(0, 1) as usize),
            (b'X', []) => self.erase_chars(csi.get(0, 1) as usize),
            (b'Z', []) => self.horizontal_tab_back(csi.get(0, 1) as usize),
            (b'@', []) => self.insert_blanks(csi.get(0, 1) as usize),
            (b'b', []) => self.repeat_previous(csi.get(0, 1) as usize),
            (b'c', []) => {
                // DA1: VT220 with ANSI color.
                self.outbound.extend_from_slice(b"\x1b[?62;22c");
            }
            (b'c', [b'>']) => self.outbound.extend_from_slice(b"\x1b[>1;0;0c"),
            (b'c', [b'=']) => self.outbound.extend_from_slice(b"\x1bP!|00000000\x1b\\"),
            (b'd', []) => {
                // VPA.
                let row = csi.get(0, 1) as usize;
                let origin = self.modes.get(Mode::Origin);
                let s = self.screen_mut();
                let (off, max) = if origin {
                    (s.scroll_top, s.scroll_bottom + 1)
                } else {
                    (0, s.rows)
                };
                s.cursor.y = (row + off).min(max) - 1;
                s.cursor.pending_wrap = false;
                self.dirty = true;
            }
            (b'g', []) => match csi.get_allow_zero(0, 0) {
                0 => {
                    let x = self.screen().cursor.x;
                    self.tabstops.unset(x);
                }
                3 => self.tabstops.clear_all(),
                _ => {}
            },
            (b'h', [b'?']) => {
                for i in 0..csi.params_len {
                    self.set_mode(csi.params[i], false, true);
                }
            }
            (b'l', [b'?']) => {
                for i in 0..csi.params_len {
                    self.set_mode(csi.params[i], false, false);
                }
            }
            (b'h', []) => {
                for i in 0..csi.params_len {
                    self.set_mode(csi.params[i], true, true);
                }
            }
            (b'l', []) => {
                for i in 0..csi.params_len {
                    self.set_mode(csi.params[i], true, false);
                }
            }
            (b'm', [b'>']) => {
                // XTMODKEYS: CSI > Pp ; Pv m
                if csi.get_allow_zero(0, 0) == 4 {
                    self.modify_other_keys = csi.get_allow_zero(1, 0).min(2) as u8;
                }
            }
            (b'm', [b'?']) => {
                // XTQMODKEYS: CSI ? Pp m
                if csi.get_allow_zero(0, 0) == 4 {
                    let s = format!("\x1b[>4;{}m", self.modify_other_keys);
                    self.outbound.extend_from_slice(s.as_bytes());
                }
            }
            (b'm', []) => self.apply_sgr(csi),
            (b'n', []) if !private => match csi.get_allow_zero(0, 0) {
                5 => self.outbound.extend_from_slice(b"\x1b[0n"),
                6 => {
                    // CPR is origin-relative under DECOM.
                    let origin = self.modes.get(Mode::Origin);
                    let s = self.screen();
                    let (row, col) = if origin {
                        (
                            s.cursor.y - s.scroll_top + 1,
                            s.cursor.x - s.left_margin.min(s.cursor.x) + 1,
                        )
                    } else {
                        (s.cursor.y + 1, s.cursor.x + 1)
                    };
                    let reply = format!("\x1b[{};{}R", row, col);
                    self.outbound.extend_from_slice(reply.as_bytes());
                }
                _ => {}
            },
            (b'n', [b'?']) => {
                if csi.get_allow_zero(0, 0) == 6 {
                    let s = self.screen();
                    let reply = format!("\x1b[?{};{}R", s.cursor.y + 1, s.cursor.x + 1);
                    self.outbound.extend_from_slice(reply.as_bytes());
                }
            }
            (b'p', [b'!']) => self.soft_reset(),
            (b'p', [b'$']) => {
                // DECRQM ANSI.
                let value = csi.get_allow_zero(0, 0);
                let report = self.modes.report(value, true);
                let s = format!("\x1b[{};{}$y", value, report.param());
                self.outbound.extend_from_slice(s.as_bytes());
            }
            (b'p', [b'?', b'$']) | (b'p', [b'$', b'?']) => {
                // DECRQM DEC private.
                let value = csi.get_allow_zero(0, 0);
                let report = self.modes.report(value, false);
                let s = format!("\x1b[?{};{}$y", value, report.param());
                self.outbound.extend_from_slice(s.as_bytes());
            }
            (b'q', [b' ']) => {
                self.cursor_style = match csi.get_allow_zero(0, 0) {
                    0 => CursorStyle::Default,
                    1 => CursorStyle::BlinkingBlock,
                    2 => CursorStyle::SteadyBlock,
                    3 => CursorStyle::BlinkingUnderline,
                    4 => CursorStyle::SteadyUnderline,
                    5 => CursorStyle::BlinkingBar,
                    6 => CursorStyle::SteadyBar,
                    _ => return,
                };
                self.dirty = true;
            }
            (b'q', [b'>']) => {
                // XTVERSION.
                self.outbound
                    .extend_from_slice(b"\x1bP>|mpterm 0.1.0\x1b\\");
            }
            (b'r', []) if !private => {
                let s = self.screen();
                let (top, bottom) = (csi.get(0, 1) as usize, {
                    let b = csi.get_allow_zero(1, 0) as usize;
                    if b == 0 {
                        s.rows
                    } else {
                        b.min(s.rows)
                    }
                });
                let top = top.max(1);
                if top < bottom {
                    let s = self.screen_mut();
                    s.scroll_top = top - 1;
                    s.scroll_bottom = bottom - 1;
                    self.set_cursor_pos(1, 1);
                    self.dirty = true;
                }
            }
            (b's', []) if !private => {
                if self.modes.get(Mode::EnableLeftAndRightMargin) {
                    // DECSLRM.
                    let s = self.screen();
                    let left = csi.get(0, 1) as usize;
                    let right = {
                        let r = csi.get_allow_zero(1, 0) as usize;
                        if r == 0 {
                            s.cols
                        } else {
                            r.min(s.cols)
                        }
                    };
                    let left = left.max(1);
                    if left < right {
                        let s = self.screen_mut();
                        s.left_margin = left - 1;
                        s.right_margin = right - 1;
                        self.set_cursor_pos(1, 1);
                        self.dirty = true;
                    }
                } else {
                    // SCOSC.
                    self.save_cursor();
                }
            }
            (b's', [b'?']) => {
                // XTSAVE.
                for i in 0..csi.params_len {
                    if let Some(mode) = mode_from_int(csi.params[i], false) {
                        self.modes.save(mode);
                    }
                }
            }
            (b'r', [b'?']) => {
                // XTRESTORE.
                for i in 0..csi.params_len {
                    if let Some(mode) = mode_from_int(csi.params[i], false) {
                        let v = self.modes.restore(mode);
                        // Screen-switch pseudo modes never land here (they
                        // are not in the registry), so a plain set is right.
                        self.modes.set(mode, v);
                    }
                }
                self.dirty = true;
            }
            (b't', []) => match csi.get_allow_zero(0, 0) {
                14 => {
                    // Text area size in pixels: unknown, report cells*8x16
                    // (apps mostly want a nonzero sane value).
                    let s = format!("\x1b[4;{};{}t", self.rows() * 16, self.cols() * 8);
                    self.outbound.extend_from_slice(s.as_bytes());
                }
                16 => {
                    self.outbound.extend_from_slice(b"\x1b[6;16;8t");
                }
                18 => {
                    let s = format!("\x1b[8;{};{}t", self.rows(), self.cols());
                    self.outbound.extend_from_slice(s.as_bytes());
                }
                _ => {}
            },
            (b'u', []) if csi.intermediates_len == 0 && !private => {
                self.restore_cursor();
            }
            // Kitty keyboard protocol.
            (b'u', [b'?']) => {
                let s = format!("\x1b[?{}u", self.kitty_flags());
                self.outbound.extend_from_slice(s.as_bytes());
            }
            (b'u', [b'>']) => {
                let flags = csi.get_allow_zero(0, 0).min(31) as u8;
                let stack = self.kitty_stack_mut();
                if stack.len() >= 8 {
                    stack.remove(0);
                }
                stack.push(flags);
            }
            (b'u', [b'<']) => {
                let n = csi.get(0, 1) as usize;
                let stack = self.kitty_stack_mut();
                for _ in 0..n {
                    if stack.pop().is_none() {
                        break;
                    }
                }
            }
            (b'u', [b'=']) => {
                let flags = csi.get_allow_zero(0, 0).min(31) as u8;
                let mode = csi.get(1, 1);
                let stack = self.kitty_stack_mut();
                let current = stack.last().copied().unwrap_or(0);
                let new = match mode {
                    1 => flags,
                    2 => current | flags,
                    3 => current & !flags,
                    _ => return,
                };
                if let Some(last) = stack.last_mut() {
                    *last = new;
                } else {
                    stack.push(new);
                }
            }
            _ => {}
        }
    }

    // ==================================================================
    // ESC dispatch
    // ==================================================================

    fn esc_dispatch(&mut self, esc: &Esc) {
        match (esc.intermediates(), esc.final_byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([b'#'], b'8') => self.decaln(),
            ([], b'D') => self.index(),
            ([], b'E') => {
                self.index();
                self.carriage_return();
            }
            ([], b'H') => {
                let x = self.screen().cursor.x;
                self.tabstops.set(x);
            }
            ([], b'M') => self.reverse_index(),
            ([], b'N') => {
                self.screen_mut().charsets.single_shift = Some(Slot::G2);
            }
            ([], b'O') => {
                self.screen_mut().charsets.single_shift = Some(Slot::G3);
            }
            ([], b'c') => self.full_reset(),
            ([], b'=') => self.modes.set(Mode::KeypadKeys, true),
            ([], b'>') => self.modes.set(Mode::KeypadKeys, false),
            // Charset designations.
            ([i], final_byte) if matches!(i, b'(' | b')' | b'*' | b'+') => {
                let slot = match i {
                    b'(' => Slot::G0,
                    b')' => Slot::G1,
                    b'*' => Slot::G2,
                    _ => Slot::G3,
                };
                let set = match final_byte {
                    b'B' => Charset::Ascii,
                    b'A' => Charset::British,
                    b'0' => Charset::DecSpecial,
                    _ => Charset::Utf8,
                };
                self.screen_mut().charsets.charsets[slot as usize] = set;
            }
            ([], b'n') => self.screen_mut().charsets.gl = Slot::G2, // LS2
            ([], b'o') => self.screen_mut().charsets.gl = Slot::G3, // LS3
            _ => {}
        }
    }

    // ==================================================================
    // OSC dispatch
    // ==================================================================

    fn osc_dispatch(&mut self, cmd: OscCommand) {
        match cmd {
            OscCommand::ChangeWindowTitle(title) => {
                self.title = title.clone();
                self.events.push(TermEvent::TitleChanged(title));
            }
            OscCommand::ChangeWindowIcon(_) => {}
            OscCommand::Colors { ops, terminator } => self.color_ops(ops, terminator),
            OscCommand::Hyperlink { id: _, uri } => {
                self.hyperlinks.push(uri);
                self.current_hyperlink = self.hyperlinks.len() as u32;
            }
            OscCommand::HyperlinkEnd => self.current_hyperlink = 0,
            OscCommand::ClipboardContents {
                kind,
                data,
                terminator,
            } => {
                if data == "?" {
                    // Clipboard reads are denied (secure default): reply
                    // with an empty payload.
                    let mut reply = format!("\x1b]52;{};", kind as char).into_bytes();
                    reply.extend_from_slice(terminator.bytes());
                    self.outbound.extend_from_slice(&reply);
                } else if let Some(text) = base64_decode(data.as_bytes()) {
                    if let Ok(text) = String::from_utf8(text) {
                        self.events.push(TermEvent::ClipboardSet { kind, text });
                    }
                }
            }
            OscCommand::ReportPwd { url } => {
                self.pwd = url.clone();
                self.events.push(TermEvent::PwdChanged(url));
            }
            OscCommand::PromptStart { .. } => {
                let y = self.screen().cursor.y;
                self.screen_mut().row_mut(y).semantic = SemanticPrompt::Prompt;
            }
            OscCommand::PromptEnd => {
                let y = self.screen().cursor.y;
                self.screen_mut().row_mut(y).semantic = SemanticPrompt::Input;
            }
            OscCommand::EndOfInput => {
                let y = self.screen().cursor.y;
                self.screen_mut().row_mut(y).semantic = SemanticPrompt::Output;
            }
            OscCommand::EndOfCommand { .. } => {}
            OscCommand::ShowDesktopNotification { title, body } => {
                self.events.push(TermEvent::Notification { title, body });
            }
        }
    }

    fn color_ops(&mut self, ops: Vec<ColorOp>, terminator: OscTerminator) {
        for op in ops {
            match op {
                ColorOp::Set(kind, rgb) => {
                    self.dirty = true;
                    match kind {
                        ColorKind::Palette(i) => self.palette[i as usize] = rgb,
                        ColorKind::Foreground => self.default_fg = rgb,
                        ColorKind::Background => self.default_bg = rgb,
                        ColorKind::Cursor => self.cursor_color = Some(rgb),
                    }
                }
                ColorOp::Reset(kind) => {
                    self.dirty = true;
                    match kind {
                        ColorKind::Palette(i) => {
                            self.palette[i as usize] = self.base_palette[i as usize]
                        }
                        ColorKind::Foreground => self.default_fg = self.base_fg,
                        ColorKind::Background => self.default_bg = self.base_bg,
                        ColorKind::Cursor => self.cursor_color = None,
                    }
                }
                ColorOp::Query(kind) => {
                    let (num, arg, color) = match kind {
                        ColorKind::Palette(i) => {
                            (4u16, Some(i), self.palette[i as usize])
                        }
                        ColorKind::Foreground => (10, None, self.default_fg),
                        ColorKind::Background => (11, None, self.default_bg),
                        ColorKind::Cursor => {
                            (12, None, self.cursor_color.unwrap_or(self.default_fg))
                        }
                    };
                    let mut reply = match arg {
                        Some(i) => format!("\x1b]{};{};{}", num, i, encode_color_reply(color)),
                        None => format!("\x1b]{};{}", num, encode_color_reply(color)),
                    }
                    .into_bytes();
                    reply.extend_from_slice(terminator.bytes());
                    self.outbound.extend_from_slice(&reply);
                }
            }
        }
    }

    // ==================================================================
    // DCS (only replies that keep apps from hanging: DECRQSS, XTGETTCAP)
    // ==================================================================

    fn dcs_hook(&mut self, dcs: &Dcs) {
        self.dcs_kind = match (dcs.final_byte, {
            let i = dcs.intermediates_len;
            if i > 0 {
                dcs.intermediates[0]
            } else {
                0
            }
        }) {
            (b'q', b'$') => DcsKind::Decrqss,
            (b'q', b'+') => DcsKind::Xtgettcap,
            _ => DcsKind::Ignore,
        };
        self.dcs_buf.clear();
    }

    fn dcs_put(&mut self, byte: u8) {
        if self.dcs_kind != DcsKind::Ignore && self.dcs_buf.len() < 256 {
            self.dcs_buf.push(byte);
        }
    }

    fn dcs_unhook(&mut self) {
        match self.dcs_kind {
            DcsKind::Decrqss => {
                // Report "invalid" for everything we don't model. Apps only
                // need a well-formed answer to stop waiting.
                let mut reply: Vec<u8> = b"\x1bP0$r".to_vec();
                reply.extend_from_slice(&self.dcs_buf);
                reply.extend_from_slice(b"\x1b\\");
                self.outbound.extend_from_slice(&reply);
            }
            DcsKind::Xtgettcap => {
                self.outbound.extend_from_slice(b"\x1bP0+r\x1b\\");
            }
            DcsKind::Ignore => {}
        }
        self.dcs_kind = DcsKind::Ignore;
        self.dcs_buf.clear();
    }

    // ==================================================================
    // Resets
    // ==================================================================

    /// DECSTR.
    fn soft_reset(&mut self) {
        self.modes.set(Mode::CursorVisible, true);
        self.modes.set(Mode::Insert, false);
        self.modes.set(Mode::Origin, false);
        self.modes.set(Mode::CursorKeys, false);
        self.modes.set(Mode::KeypadKeys, false);
        self.cursor_style = CursorStyle::Default;
        let s = self.screen_mut();
        s.reset_margins();
        s.cursor.style = Default::default();
        s.cursor.pending_wrap = false;
        s.saved_cursor = None;
        s.charsets = Default::default();
        self.dirty = true;
    }

    /// RIS.
    pub fn full_reset(&mut self) {
        let (cols, rows) = (self.cols(), self.rows());
        self.primary = Screen::new(cols, rows, DEFAULT_SCROLLBACK);
        self.alternate = Screen::new(cols, rows, 0);
        self.active = ActiveScreen::Primary;
        self.modes.reset();
        self.tabstops = crate::term::tabstops::Tabstops::new(cols);
        self.palette = self.base_palette;
        self.default_fg = self.base_fg;
        self.default_bg = self.base_bg;
        self.cursor_color = None;
        self.cursor_style = CursorStyle::Default;
        self.kitty_flags_primary.clear();
        self.kitty_flags_alternate.clear();
        self.modify_other_keys = 0;
        self.hyperlinks.clear();
        self.current_hyperlink = 0;
        self.previous_printed = None;
        self.dcs_kind = DcsKind::Ignore;
        self.dcs_buf.clear();
        self.dirty = true;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DcsKind {
    Ignore,
    Decrqss,
    Xtgettcap,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::stream::Stream;

    fn term(cols: usize, rows: usize) -> (Stream, Terminal) {
        (Stream::new(), Terminal::new(cols, rows))
    }

    fn feed(s: &mut Stream, t: &mut Terminal, bytes: &[u8]) {
        s.process(bytes, t);
    }

    fn row_text(t: &Terminal, y: usize) -> String {
        t.screen().row(y).text()
    }

    #[test]
    fn print_and_wrap_pending() {
        let (mut s, mut t) = term(5, 3);
        feed(&mut s, &mut t, b"abcde");
        // Deferred wrap: cursor stays on the last column.
        assert_eq!(t.screen().cursor.x, 4);
        assert!(t.screen().cursor.pending_wrap);
        feed(&mut s, &mut t, b"f");
        assert_eq!(row_text(&t, 0), "abcde");
        assert_eq!(row_text(&t, 1), "f");
        assert!(t.screen().row(0).wrapped);
    }

    #[test]
    fn alt_screen_1049_roundtrip() {
        let (mut s, mut t) = term(20, 5);
        feed(&mut s, &mut t, b"shell line\r\n$ ");
        feed(&mut s, &mut t, b"\x1b[?1049h");
        assert_eq!(t.active, ActiveScreen::Alternate);
        feed(&mut s, &mut t, b"\x1b[2J\x1b[HTUI CONTENT");
        assert_eq!(row_text(&t, 0), "TUI CONTENT");
        feed(&mut s, &mut t, b"\x1b[?1049l");
        assert_eq!(t.active, ActiveScreen::Primary);
        assert_eq!(row_text(&t, 0), "shell line");
        assert_eq!(row_text(&t, 1), "$");
        // Cursor restored to the shell prompt position.
        assert_eq!(t.screen().cursor.y, 1);
        assert_eq!(t.screen().cursor.x, 2);
    }

    #[test]
    fn decset_modes_apply() {
        let (mut s, mut t) = term(10, 4);
        feed(&mut s, &mut t, b"\x1b[?1h\x1b[?2004h\x1b[?25l");
        assert!(t.modes.get(Mode::CursorKeys));
        assert!(t.modes.get(Mode::BracketedPaste));
        assert!(!t.modes.get(Mode::CursorVisible));
        feed(&mut s, &mut t, b"\x1b[?1l\x1b[?25h");
        assert!(!t.modes.get(Mode::CursorKeys));
        assert!(t.modes.get(Mode::CursorVisible));
    }

    #[test]
    fn linefeed_below_scroll_region_does_not_scroll_it() {
        let (mut s, mut t) = term(10, 5);
        // Region rows 1-3 (1-based), status on row 5.
        feed(&mut s, &mut t, b"\x1b[1;3r");
        feed(&mut s, &mut t, b"\x1b[1;1Htop");
        feed(&mut s, &mut t, b"\x1b[5;1Hstatus");
        // LF at the last row, below the region: cursor pinned, no scroll.
        feed(&mut s, &mut t, b"\n");
        assert_eq!(row_text(&t, 0), "top");
        assert_eq!(row_text(&t, 4), "status");
        assert_eq!(t.screen().cursor.y, 4);
    }

    #[test]
    fn scroll_region_scrolls_inside_only() {
        let (mut s, mut t) = term(10, 4);
        feed(&mut s, &mut t, b"\x1b[1;2r");
        feed(&mut s, &mut t, b"\x1b[1;1Haaa\r\nbbb");
        feed(&mut s, &mut t, b"\x1b[4;1Hbot");
        feed(&mut s, &mut t, b"\x1b[2;1H\n");
        assert_eq!(row_text(&t, 0), "bbb");
        assert_eq!(row_text(&t, 1), "");
        assert_eq!(row_text(&t, 3), "bot");
        // Ghostty behavior: a full-width region anchored at row 0 feeds
        // scrollback even when a bottom margin exists.
        assert_eq!(t.screen().scrollback.len(), 1);
        assert_eq!(t.screen().scrollback[0].text(), "aaa");
    }

    #[test]
    fn sgr_truecolor_and_reset() {
        let (mut s, mut t) = term(10, 2);
        feed(&mut s, &mut t, b"\x1b[1;38;2;10;20;30mX\x1b[0mY");
        let cell_x = t.screen().row(0).cell(0).unwrap().clone();
        assert_eq!(cell_x.style.fg_color, StyleColor::Rgb(Rgb::new(10, 20, 30)));
        assert!(cell_x.style.flags.has(StyleFlags::BOLD));
        let cell_y = t.screen().row(0).cell(1).unwrap();
        assert!(cell_y.style.is_default());
    }

    #[test]
    fn undercurl_colon_form() {
        let (mut s, mut t) = term(10, 2);
        feed(&mut s, &mut t, b"\x1b[4:3mX");
        let cell = t.screen().row(0).cell(0).unwrap();
        assert_eq!(
            cell.style.flags.underline(),
            crate::term::style::Underline::Curly
        );
        assert!(!cell.style.flags.has(StyleFlags::ITALIC));
    }

    #[test]
    fn dsr_cpr_reply() {
        let (mut s, mut t) = term(80, 24);
        feed(&mut s, &mut t, b"\x1b[12;34H\x1b[6n");
        assert_eq!(t.take_outbound(), b"\x1b[12;34R".to_vec());
    }

    #[test]
    fn da1_reply() {
        let (mut s, mut t) = term(80, 24);
        feed(&mut s, &mut t, b"\x1b[c");
        assert_eq!(t.take_outbound(), b"\x1b[?62;22c".to_vec());
    }

    #[test]
    fn osc_title_and_events() {
        let (mut s, mut t) = term(80, 24);
        feed(&mut s, &mut t, b"\x1b]2;hello world\x07");
        assert_eq!(t.title, "hello world");
        assert!(t
            .take_events()
            .contains(&TermEvent::TitleChanged("hello world".into())));
    }

    #[test]
    fn wide_char_occupies_two_cells() {
        let (mut s, mut t) = term(10, 2);
        feed(&mut s, &mut t, "漢x".as_bytes());
        let row = t.screen().row(0);
        assert_eq!(
            row.cell(0).unwrap().content,
            crate::term::page::CellContent::WideChar('漢')
        );
        assert_eq!(
            row.cell(1).unwrap().content,
            crate::term::page::CellContent::WideTail
        );
        assert_eq!(row.cell(2).unwrap().content.primary(), Some('x'));
    }

    #[test]
    fn wide_char_at_margin_wraps_with_spacer() {
        let (mut s, mut t) = term(5, 3);
        feed(&mut s, &mut t, "abcd漢".as_bytes());
        let row0 = t.screen().row(0);
        assert_eq!(
            row0.cell(4).unwrap().content,
            crate::term::page::CellContent::WideSpacerHead
        );
        assert!(row0.wrapped);
        assert_eq!(
            t.screen().row(1).cell(0).unwrap().content,
            crate::term::page::CellContent::WideChar('漢')
        );
    }

    #[test]
    fn combining_attaches_without_2027() {
        let (mut s, mut t) = term(10, 2);
        feed(&mut s, &mut t, "e\u{0301}".as_bytes());
        let cell = t.screen().row(0).cell(0).unwrap();
        match &cell.content {
            crate::term::page::CellContent::Cluster(c) => {
                assert_eq!(c.cps, vec!['e', '\u{0301}']);
                assert_eq!(c.width, 1);
            }
            other => panic!("expected cluster, got {:?}", other),
        }
        assert_eq!(t.screen().cursor.x, 1);
    }

    #[test]
    fn dec_special_graphics() {
        let (mut s, mut t) = term(10, 2);
        feed(&mut s, &mut t, b"\x1b(0qx\x1b(Bq");
        let row = t.screen().row(0);
        assert_eq!(row.cell(0).unwrap().content.primary(), Some('─'));
        assert_eq!(row.cell(1).unwrap().content.primary(), Some('│'));
        assert_eq!(row.cell(2).unwrap().content.primary(), Some('q'));
    }

    #[test]
    fn ris_resets_everything() {
        let (mut s, mut t) = term(10, 4);
        feed(&mut s, &mut t, b"\x1b[31mhello\x1b[?1049h\x1b[2;3r");
        feed(&mut s, &mut t, b"\x1bc");
        assert_eq!(t.active, ActiveScreen::Primary);
        assert_eq!(row_text(&t, 0), "");
        assert!(t.screen().cursor.style.is_default());
        assert_eq!(t.screen().scroll_top, 0);
        assert_eq!(t.screen().scroll_bottom, 3);
    }

    #[test]
    fn scrollback_grows_and_reflows() {
        let (mut s, mut t) = term(10, 3);
        for i in 0..10 {
            feed(&mut s, &mut t, format!("line{}\r\n", i).as_bytes());
        }
        assert!(t.primary.scrollback.len() >= 7);
        assert_eq!(row_text(&t, 2), "");
        // Width shrink keeps content through reflow.
        t.resize(4, 3);
        let all: Vec<String> = (0..t.primary.scrollback.len())
            .map(|i| t.primary.scrollback[i].text())
            .collect();
        assert!(all.iter().any(|l| l == "line"));
    }

    #[test]
    fn kitty_keyboard_stack() {
        let (mut s, mut t) = term(10, 3);
        feed(&mut s, &mut t, b"\x1b[>1u");
        assert_eq!(t.kitty_flags(), 1);
        feed(&mut s, &mut t, b"\x1b[?u");
        assert_eq!(t.take_outbound(), b"\x1b[?1u");
        feed(&mut s, &mut t, b"\x1b[<1u");
        assert_eq!(t.kitty_flags(), 0);
    }

    #[test]
    fn cursor_up_down_clamps_to_region() {
        let (mut s, mut t) = term(10, 6);
        feed(&mut s, &mut t, b"\x1b[2;4r\x1b[3;1H");
        // Cursor inside region: CUU clamps at region top.
        feed(&mut s, &mut t, b"\x1b[9A");
        assert_eq!(t.screen().cursor.y, 1);
        feed(&mut s, &mut t, b"\x1b[9B");
        assert_eq!(t.screen().cursor.y, 3);
    }

    #[test]
    fn origin_mode_cursor_addressing() {
        let (mut s, mut t) = term(10, 6);
        feed(&mut s, &mut t, b"\x1b[2;4r\x1b[?6h\x1b[1;1HX");
        // Origin mode: home is the region's top-left.
        assert_eq!(row_text(&t, 1), "X");
        feed(&mut s, &mut t, b"\x1b[6n");
        assert_eq!(t.take_outbound(), b"\x1b[1;2R".to_vec());
    }

    #[test]
    fn erase_line_uses_bg_and_clears_wrap() {
        let (mut s, mut t) = term(6, 3);
        feed(&mut s, &mut t, b"abcdef");
        assert!(t.screen().row(0).wrapped || t.screen().cursor.pending_wrap);
        feed(&mut s, &mut t, b"\x1b[1;3H\x1b[41m\x1b[K");
        let row = t.screen().row(0);
        assert!(!row.wrapped);
        assert_eq!(row.text(), "ab");
        let erased = row.cell(4).unwrap();
        assert_eq!(erased.style.bg_color, StyleColor::Palette(1));
    }

    #[test]
    fn insert_delete_chars_respect_margins() {
        let (mut s, mut t) = term(10, 3);
        feed(&mut s, &mut t, b"abcdefghij\x1b[1;3H\x1b[2@");
        assert_eq!(row_text(&t, 0), "ab  cdefgh");
        feed(&mut s, &mut t, b"\x1b[1;1H\x1b[2P");
        assert_eq!(row_text(&t, 0), "  cdefgh");
    }

    #[test]
    fn rep_repeats_previous_char() {
        let (mut s, mut t) = term(10, 2);
        feed(&mut s, &mut t, b"a\x1b[3b");
        assert_eq!(row_text(&t, 0), "aaaa");
    }

    #[test]
    fn decrqm_reports() {
        let (mut s, mut t) = term(10, 2);
        feed(&mut s, &mut t, b"\x1b[?2004h\x1b[?2004$p");
        assert_eq!(t.take_outbound(), b"\x1b[?2004;1$y".to_vec());
        feed(&mut s, &mut t, b"\x1b[?2026$p");
        assert_eq!(t.take_outbound(), b"\x1b[?2026;2$y".to_vec());
    }

    #[test]
    fn osc_color_query_echoes_terminator() {
        let (mut s, mut t) = term(10, 2);
        t.default_bg = Rgb::new(0x11, 0x22, 0x33);
        feed(&mut s, &mut t, b"\x1b]11;?\x07");
        assert_eq!(
            t.take_outbound(),
            b"\x1b]11;rgb:1111/2222/3333\x07".to_vec()
        );
        feed(&mut s, &mut t, b"\x1b]11;?\x1b\\");
        assert_eq!(
            t.take_outbound(),
            b"\x1b]11;rgb:1111/2222/3333\x1b\\".to_vec()
        );
    }
}

fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u8;
    for &b in input {
        if b == b'=' || b == b'\n' || b == b'\r' {
            continue;
        }
        let v = val(b)?;
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}
