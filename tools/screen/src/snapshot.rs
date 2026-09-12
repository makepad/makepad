//! Per-display safe ANSI projection of one authoritative terminal.
//!
//! Only generated cell painting and mode setters leave this module. DSR,
//! DA, OSC queries, clipboard commands and arbitrary source escape strings
//! never reach an attached terminal, so it cannot answer queries twice.
use crate::term::{
    color::Rgb,
    modes::Mode,
    page::{CellContent, Row},
    screen::{CursorStyle, Screen},
    style::{Style, StyleColor, StyleFlags},
    terminal::ActiveScreen,
    Terminal,
};
use crate::terminal::{MAX_COLS, MAX_ROWS};
use std::fmt::Write;

pub const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const BEGIN: &str =
    "\x1b[?2026h\x1b[?25l\x1b[?6l\x1b[?69l\x1b[r\x1b[4l\x1b[20l\x1b[?7h\x1b(B\x1b)B\x0f";
const END: &str = "\x1b[?2026l";

#[derive(Default)]
pub struct Projection {
    previous: Option<Previous>,
    // Retain applied colors across invalidation/resizes so a repaint can
    // restore overrides that the child has since removed.
    colors: Colors,
    /// An oversized current grid is reported visibly, never truncated into
    /// an invalid escape sequence. The canonical terminal remains intact.
    pub error: Option<String>,
}

struct Previous {
    viewport: (usize, usize),
    canonical: (usize, usize),
    history_start: u64,
    history_end: u64,
    history_epoch: u64,
    active: ActiveScreen,
    primary: Vec<String>,
    alternate: Vec<String>,
    metadata: String,
    input: String,
    bells: u64,
}

impl Projection {
    pub fn invalidate(&mut self) {
        self.previous = None;
        self.error = None;
    }

    pub fn render(
        &mut self,
        terminal: &Terminal,
        bells: u64,
        history_epoch: u64,
        cols: usize,
        rows: usize,
    ) -> Vec<u8> {
        let viewport = (cols.clamp(2, MAX_COLS), rows.clamp(2, MAX_ROWS));
        let canonical = (terminal.cols(), terminal.rows());
        let history_start = terminal.primary.evicted;
        let history_end = history_start.saturating_add(terminal.primary.scrollback.len() as u64);
        let primary = grid(&terminal.primary, viewport);
        let alternate = grid(&terminal.alternate, viewport);
        let metadata = metadata(terminal);
        let input = input_state(terminal, viewport);
        let current = Previous {
            viewport,
            canonical,
            history_start,
            history_end,
            history_epoch,
            active: terminal.active,
            primary,
            alternate,
            metadata,
            input,
            bells,
        };
        let old = self.previous.as_ref();
        let colors = Colors::from_terminal(terminal);
        let color_updates = colors.changes(&self.colors);
        let full = old.is_none_or(|old| {
            old.viewport != viewport
                || old.canonical != canonical
                || old.history_epoch != history_epoch
                || history_start < old.history_start
                || history_end < old.history_end
                || history_start > old.history_end
                || (history_start > old.history_start && history_end == old.history_end)
        });
        let mut output = if full {
            full_snapshot(terminal, &current, &color_updates)
        } else {
            delta(terminal, old.unwrap(), &current, &color_updates)
        };
        if output.len() > MAX_SNAPSHOT_BYTES {
            // Extremely long grapheme clusters can exceed a byte bound even
            // with bounded rows/columns. Never send a partial ANSI stream.
            self.error = Some("The current terminal image exceeds the 16 MiB display bound".into());
            output = format!(
                "{BEGIN}\x1b[?1049l\x1b[0m\x1b[2J\x1b[H{}\x1b[?25l{END}",
                self.error.as_deref().unwrap()
            );
            self.previous = None;
        } else {
            self.error = None;
            self.previous = Some(current);
            self.colors = colors;
        }
        output.into_bytes()
    }
}

fn full_snapshot(terminal: &Terminal, current: &Previous, color_updates: &str) -> String {
    let mut output = String::from(BEGIN);
    painting_modes(&mut output, terminal);
    // Explicit attach replaces the managed client's terminal history. This
    // does not affect any other client's viewport or the authoritative PTY.
    output.push_str("\x1b[?1049l\x1b[?47l\x1b[0m\x1b[3J\x1b[2J");
    output.push_str(&current.metadata);
    output.push_str(color_updates);
    let visible_bytes = current.primary.iter().map(String::len).sum::<usize>()
        + if current.active == ActiveScreen::Alternate {
            current.alternate.iter().map(String::len).sum::<usize>() + 16
        } else {
            0
        };
    let reserve = output.len() + visible_bytes + current.input.len() + END.len() + 128;
    let mut remaining = MAX_SNAPSHOT_BYTES.saturating_sub(reserve);
    let mut history = Vec::new();
    // Keep a recent complete-row tail; never crop ANSI or a UTF-8 character.
    for row in terminal.primary.scrollback.iter().rev() {
        let encoded = history_row(row, terminal.primary.cols, current.viewport);
        if encoded.len() > remaining {
            break;
        }
        remaining -= encoded.len();
        history.push(encoded);
    }
    for row in history.into_iter().rev() {
        output.push_str(&row);
    }
    for row in &current.primary {
        output.push_str(row);
    }
    if current.active == ActiveScreen::Alternate {
        output.push_str("\x1b[?1049h");
        for row in &current.alternate {
            output.push_str(row);
        }
    }
    output.push_str(&current.input);
    output.push_str(END);
    output
}

fn delta(terminal: &Terminal, old: &Previous, current: &Previous, color_updates: &str) -> String {
    let added = current.history_end.saturating_sub(old.history_end);
    let switching = old.active != current.active;
    let primary_changed = old.primary != current.primary;
    let alternate_changed = old.alternate != current.alternate;
    let body_changed = match current.active {
        ActiveScreen::Primary => primary_changed,
        ActiveScreen::Alternate => alternate_changed,
    };
    if added == 0
        && !switching
        && !body_changed
        && old.metadata == current.metadata
        && old.input == current.input
        && old.bells == current.bells
        && color_updates.is_empty()
    {
        return String::new();
    }
    let mut output = String::from(BEGIN);
    painting_modes(&mut output, terminal);
    output.push_str(color_updates);
    if old.metadata != current.metadata {
        output.push_str(&current.metadata);
    }
    if added != 0 {
        // Paint the exact row that scrolled off at the top, then scroll it
        // into the client's history. Repaint its small visible grid once.
        if old.active == ActiveScreen::Alternate {
            output.push_str("\x1b[?1049l");
        }
        let skip = old.history_end.saturating_sub(current.history_start) as usize;
        let reserve = current.primary.iter().map(String::len).sum::<usize>()
            + if current.active == ActiveScreen::Alternate {
                current.alternate.iter().map(String::len).sum::<usize>() + 16
            } else {
                0
            }
            + current.input.len()
            + END.len()
            + 1;
        for row in terminal.primary.scrollback.iter().skip(skip) {
            let encoded = history_row(row, terminal.primary.cols, current.viewport);
            if output
                .len()
                .saturating_add(encoded.len())
                .saturating_add(reserve)
                > MAX_SNAPSHOT_BYTES
            {
                return full_snapshot(terminal, current, color_updates);
            }
            output.push_str(&encoded);
        }
        for row in &current.primary {
            output.push_str(row);
        }
        if current.active == ActiveScreen::Alternate {
            output.push_str("\x1b[?1049h");
            for row in &current.alternate {
                output.push_str(row);
            }
        }
    } else {
        if switching {
            output.push_str(if current.active == ActiveScreen::Alternate {
                "\x1b[?1049h"
            } else {
                "\x1b[?1049l"
            });
        }
        let (previous, next) = if current.active == ActiveScreen::Alternate {
            (&old.alternate, &current.alternate)
        } else {
            (&old.primary, &current.primary)
        };
        let mut wrapped_into_next = false;
        for (index, row) in next.iter().enumerate() {
            let paint = switching || wrapped_into_next || previous.get(index) != Some(row);
            if paint {
                output.push_str(row);
            }
            wrapped_into_next = paint
                && terminal.cols() == current.viewport.0
                && index + 1 < current.viewport.1
                && terminal
                    .screen()
                    .active
                    .get(index)
                    .is_some_and(|row| row.wrapped);
        }
    }
    if old.bells != current.bells {
        output.push('\x07');
    }
    output.push_str(&current.input);
    output.push_str(END);
    if output.len() > MAX_SNAPSHOT_BYTES {
        // A single parser pass may scroll thousands of richly styled lines.
        // Rebase the display on a bounded complete snapshot instead.
        return full_snapshot(terminal, current, color_updates);
    }
    output
}

fn painting_modes(output: &mut String, terminal: &Terminal) {
    output.push_str(if terminal.modes.get(Mode::GraphemeCluster) {
        "\x1b[?2027h"
    } else {
        "\x1b[?2027l"
    });
}

fn grid(screen: &Screen, viewport: (usize, usize)) -> Vec<String> {
    (0..viewport.1)
        .map(|index| {
            let mut output = format!("\x1b[{};1H\x1b[0m\x1b[2K", index + 1);
            if let Some(row) = screen.active.get(index) {
                paint_row(&mut output, row, screen.cols.min(viewport.0));
                // Preserve soft wraps when the display and canonical widths
                // match. The next row is repainted after the wrap marker.
                if row.wrapped && screen.cols == viewport.0 && index + 1 < viewport.1 {
                    mark_wrap(&mut output, row, screen.cols);
                }
            }
            output
        })
        .collect()
}

fn history_row(row: &Row, canonical_cols: usize, viewport: (usize, usize)) -> String {
    let mut output = String::from("\x1b[1;1H\x1b[0m\x1b[2K");
    paint_row(&mut output, row, canonical_cols.min(viewport.0));
    if row.wrapped && canonical_cols == viewport.0 {
        mark_wrap(&mut output, row, canonical_cols);
    }
    let _ = write!(output, "\x1b[0m\x1b[{};1H\r\n", viewport.1);
    output
}

fn mark_wrap(output: &mut String, row: &Row, cols: usize) {
    // paint_row leaves the cursor immediately after its last cell. Fill any
    // unwritten tail so one harmless next character establishes soft wrap.
    for _ in row.cells.len().min(cols)..cols {
        output.push(' ');
    }
    output.push(' ');
}

fn paint_row(output: &mut String, row: &Row, cols: usize) {
    let mut previous = Style::default();
    for (column, cell) in row.cells.iter().take(cols).enumerate() {
        if matches!(cell.content, CellContent::WideTail) {
            continue;
        }
        if cell.style != previous {
            sgr(output, cell.style);
            previous = cell.style;
        }
        if cell.content.width() == 2 && column + 1 >= cols {
            output.push(' ');
            continue;
        }
        match &cell.content {
            CellContent::Char(ch) | CellContent::WideChar(ch) => printable(output, *ch),
            CellContent::Cluster(cluster) => {
                for ch in &cluster.cps {
                    printable(output, *ch);
                }
            }
            _ => output.push(' '),
        }
    }
}

fn printable(output: &mut String, ch: char) {
    if ch.is_control() {
        output.push('�');
    } else {
        output.push(ch);
    }
}

fn sgr(output: &mut String, style: Style) {
    output.push_str("\x1b[0");
    for (flag, number) in [
        (StyleFlags::BOLD, 1),
        (StyleFlags::FAINT, 2),
        (StyleFlags::ITALIC, 3),
        (StyleFlags::BLINK, 5),
        (StyleFlags::INVERSE, 7),
        (StyleFlags::INVISIBLE, 8),
        (StyleFlags::STRIKETHROUGH, 9),
        (StyleFlags::OVERLINE, 53),
    ] {
        if style.flags.has(flag) {
            let _ = write!(output, ";{number}");
        }
    }
    let underline = style.flags.underline() as u8;
    if underline != 0 {
        let _ = write!(output, ";4:{underline}");
    }
    for (prefix, color) in [
        (38, style.fg_color),
        (48, style.bg_color),
        (58, style.underline_color),
    ] {
        match color {
            StyleColor::None => {}
            StyleColor::Palette(index) => {
                let _ = write!(output, ";{prefix};5;{index}");
            }
            StyleColor::Rgb(rgb) => {
                let _ = write!(output, ";{prefix};2;{};{};{}", rgb.r, rgb.g, rgb.b);
            }
        }
    }
    output.push('m');
}

struct Colors {
    palette: [Option<Rgb>; 256],
    dynamic: [Option<Rgb>; 3],
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            palette: [None; 256],
            dynamic: [None; 3],
        }
    }
}

impl Colors {
    fn from_terminal(terminal: &Terminal) -> Self {
        Self {
            palette: std::array::from_fn(|index| {
                terminal.palette_overrides[index].then_some(terminal.palette[index])
            }),
            dynamic: [
                terminal.foreground_override.then_some(terminal.default_fg),
                terminal.background_override.then_some(terminal.default_bg),
                terminal.cursor_color,
            ],
        }
    }

    fn changes(&self, old: &Self) -> String {
        let mut output = String::new();
        for (index, (color, previous)) in self.palette.iter().zip(&old.palette).enumerate() {
            if color == previous {
                continue;
            }
            if let Some(rgb) = color {
                let _ = write!(
                    output,
                    "\x1b]4;{index};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                    rgb.r, rgb.g, rgb.b
                );
            } else {
                let _ = write!(output, "\x1b]104;{index}\x1b\\");
            }
        }
        for (index, (color, previous)) in self.dynamic.iter().zip(&old.dynamic).enumerate() {
            if color == previous {
                continue;
            }
            if let Some(rgb) = color {
                let _ = write!(
                    output,
                    "\x1b]{};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
                    10 + index,
                    rgb.r,
                    rgb.g,
                    rgb.b
                );
            } else {
                let _ = write!(output, "\x1b]{}\x1b\\", 110 + index);
            }
        }
        output
    }
}

fn metadata(terminal: &Terminal) -> String {
    let mut output = String::new();
    let title: String = terminal
        .title
        .chars()
        .filter(|ch| !ch.is_control())
        .take(4096)
        .collect();
    let _ = write!(output, "\x1b]2;{title}\x1b\\");
    if !terminal.pwd.is_empty() {
        let pwd: String = terminal
            .pwd
            .chars()
            .filter(|ch| !ch.is_control())
            .take(4096)
            .collect();
        let _ = write!(output, "\x1b]7;{pwd}\x1b\\");
    }
    output
}

fn input_state(terminal: &Terminal, viewport: (usize, usize)) -> String {
    let mut output = String::from("\x1b[0m\x1b[?2031l\x1b[?2048l");
    for mode in [
        Mode::DisableKeyboard,
        Mode::Linefeed,
        Mode::CursorKeys,
        Mode::KeypadKeys,
        Mode::BackarrowKeyMode,
        Mode::ReverseColors,
        Mode::CursorBlinking,
        Mode::MouseEventX10,
        Mode::MouseEventNormal,
        Mode::MouseEventButton,
        Mode::MouseEventAny,
        Mode::FocusEvent,
        Mode::MouseFormatUtf8,
        Mode::MouseFormatSgr,
        Mode::MouseAlternateScroll,
        Mode::MouseFormatUrxvt,
        Mode::MouseFormatSgrPixels,
        Mode::IgnoreKeypadWithNumlock,
        Mode::AltEscPrefix,
        Mode::AltSendsEscape,
        Mode::BracketedPaste,
        Mode::GraphemeCluster,
    ] {
        let (number, ansi, _) = crate::term::modes::mode_entry(mode);
        let _ = write!(
            output,
            "\x1b[{}{number}{}",
            if ansi { "" } else { "?" },
            if terminal.modes.get(mode) { 'h' } else { 'l' }
        );
    }
    output.push_str(if terminal.modes.get(Mode::KeypadKeys) {
        "\x1b="
    } else {
        "\x1b>"
    });
    let _ = write!(
        output,
        "\x1b[={};1u\x1b[>4;{}m",
        terminal.kitty_flags(),
        terminal.modify_other_keys
    );
    let cursor = &terminal.screen().cursor;
    if cursor.x < viewport.0 && cursor.y < viewport.1 {
        let _ = write!(output, "\x1b[{};{}H", cursor.y + 1, cursor.x + 1);
        if terminal.modes.get(Mode::CursorVisible) {
            output.push_str("\x1b[?25h");
        }
    }
    let style = match terminal.cursor_style {
        CursorStyle::Default => 0,
        CursorStyle::BlinkingBlock => 1,
        CursorStyle::SteadyBlock => 2,
        CursorStyle::BlinkingUnderline => 3,
        CursorStyle::SteadyUnderline => 4,
        CursorStyle::BlinkingBar => 5,
        CursorStyle::SteadyBar => 6,
    };
    let _ = write!(output, "\x1b[{style} q");
    output
}
