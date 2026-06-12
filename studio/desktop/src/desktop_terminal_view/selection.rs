use crate::desktop_terminal_view::DesktopTerminalView;
use crate::makepad_widgets::*;
use makepad_studio_protocol::hub_protocol::TerminalFramebuffer;

impl DesktopTerminalView {
    pub(super) fn pick(&self, abs: Vec2d) -> (usize, usize) {
        let frame = match self.last_frame.as_ref() {
            Some(f) => f,
            None => return (0, 0),
        };
        let cols = (frame.cols as usize).max(1);
        let total_rows = frame.total_lines.max(1);
        let (cell_width, cell_height) = self.cell_metrics();
        let local_x = abs.x - self.unscrolled_rect.pos.x - self.pad_x;
        let local_y =
            abs.y - self.unscrolled_rect.pos.y - self.pad_y + self.current_scroll_pixels();

        let col = (local_x / cell_width).floor().max(0.0) as usize;
        let row = (local_y / cell_height).floor().max(0.0) as usize;
        let col = col.min(cols.saturating_sub(1));
        let row = row.min(total_rows.saturating_sub(1));
        (row, col)
    }

    pub(super) fn word_kind(ch: char) -> Option<bool> {
        if ch == '\0' || ch.is_whitespace() {
            None
        } else {
            Some(ch.is_alphanumeric() || ch == '_')
        }
    }

    pub(super) fn word_range_at_in_frame(
        frame: &TerminalFramebuffer,
        row: usize,
        col: usize,
    ) -> Option<(usize, usize)> {
        let cols = frame.cols as usize;
        let rows = frame.rows as usize;
        let frame_row = row.checked_sub(frame.top_row)?;
        if frame_row >= rows || cols == 0 {
            return None;
        }
        let col = col.min(cols.saturating_sub(1));
        let ch = Self::frame_char(frame, frame_row, col)?;
        let kind = Self::word_kind(ch)?;

        let mut start = col;
        while start > 0 {
            if let Some(prev_ch) = Self::frame_char(frame, frame_row, start - 1) {
                if Self::word_kind(prev_ch) != Some(kind) {
                    break;
                }
            } else {
                break;
            }
            start -= 1;
        }

        let mut end = col + 1;
        while end < cols {
            if let Some(next_ch) = Self::frame_char(frame, frame_row, end) {
                if Self::word_kind(next_ch) != Some(kind) {
                    break;
                }
            } else {
                break;
            }
            end += 1;
        }
        Some((start, end))
    }

    pub(super) fn frame_char(
        frame: &TerminalFramebuffer,
        frame_row: usize,
        col: usize,
    ) -> Option<char> {
        let cols = frame.cols as usize;
        let idx = (frame_row * cols + col) * 10;
        if idx + 9 >= frame.cells.len() {
            return None;
        }
        let codepoint = u32::from_le_bytes([
            frame.cells[idx],
            frame.cells[idx + 1],
            frame.cells[idx + 2],
            frame.cells[idx + 3],
        ]);
        char::from_u32(codepoint)
    }

    pub(super) fn selection_ordered(&self) -> Option<((usize, usize), (usize, usize))> {
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

    pub(super) fn is_cell_selected(&self, row: usize, col: usize) -> bool {
        let Some(((start_row, start_col), (end_row, end_col))) = self.selection_ordered() else {
            return false;
        };
        if row < start_row || row > end_row {
            return false;
        }
        if start_row == end_row {
            return col >= start_col && col < end_col;
        }
        if row == start_row {
            return col >= start_col;
        }
        if row == end_row {
            return col < end_col;
        }
        true
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        let frame = self.last_frame.as_ref()?;
        let cols = frame.cols as usize;
        if cols == 0 {
            return None;
        }
        let ((start_row, start_col), (end_row, end_col)) = self.selection_ordered()?;

        let mut out = String::new();
        for row in start_row..=end_row {
            let frame_row = match row.checked_sub(frame.top_row) {
                Some(fr) if fr < frame.rows as usize => fr,
                _ => {
                    if row < end_row {
                        out.push('\n');
                    }
                    continue;
                }
            };
            let from_col = if row == start_row { start_col } else { 0 };
            let to_col_exclusive = if row == end_row { end_col } else { cols };
            if from_col >= to_col_exclusive {
                continue;
            }
            let mut line = String::new();
            for col in from_col..to_col_exclusive {
                if let Some(ch) = Self::frame_char(frame, frame_row, col) {
                    if ch != '\0' {
                        line.push(ch);
                    }
                }
            }
            let line = line.trim_end();
            if row < end_row {
                out.push_str(line);
                out.push('\n');
            } else {
                out.push_str(line);
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }
}
