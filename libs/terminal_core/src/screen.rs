use crate::cell::{Cell, Style};
use crate::grid::Grid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

impl Default for CursorShape {
    fn default() -> Self {
        CursorShape::Block
    }
}

#[derive(Clone, Debug)]
pub struct Cursor {
    pub x: usize,
    pub y: usize,
    pub style: Style,
    pub pending_wrap: bool,
    pub visible: bool,
    pub shape: CursorShape,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            style: Style::default(),
            pending_wrap: false,
            visible: true,
            shape: CursorShape::Block,
        }
    }
}

#[derive(Clone, Debug)]
struct SavedCursor {
    x: usize,
    y: usize,
    style: Style,
    pending_wrap: bool,
}

/// A single terminal screen (primary or alternate).
pub struct Screen {
    pub grid: Grid,
    pub cursor: Cursor,
    saved_cursor: Option<SavedCursor>,

    // Scroll region (inclusive top, exclusive bottom)
    pub scroll_top: usize,
    pub scroll_bottom: usize,

    // Scrollback (primary screen only)
    scrollback: Vec<Vec<Cell>>,
    pub scrollback_wrapped: Vec<bool>,
    pub max_scrollback: usize,

    // Tab stops
    pub tabstops: Vec<bool>,

    // Highest grid row that has been written to (0-based, inclusive).
    // Tracks content extent independent of where the cursor happens to be.
    pub high_water_row: usize,
    // Rows recently pulled from scrollback during grows. If a following shrink
    // would normally trim only bottom rows (copy_start == 0), we first push
    // this many rows back into scrollback from the top to make grow->shrink
    // round-trips stable for TUI layouts on the primary screen.
    bottom_trimmed_rows: usize,
}

impl Screen {
    pub fn new(cols: usize, rows: usize, with_scrollback: bool) -> Self {
        let mut tabstops = vec![false; cols];
        for i in (0..cols).step_by(8) {
            tabstops[i] = true;
        }

        Self {
            grid: Grid::new(cols, rows),
            cursor: Cursor::default(),
            saved_cursor: None,
            scroll_top: 0,
            scroll_bottom: rows,
            scrollback: Vec::new(),
            scrollback_wrapped: Vec::new(),
            max_scrollback: if with_scrollback { 10000 } else { 0 },
            tabstops,
            high_water_row: 0,
            bottom_trimmed_rows: 0,
        }
    }

    pub fn cols(&self) -> usize {
        self.grid.cols
    }

    pub fn rows(&self) -> usize {
        self.grid.rows
    }

    /// Move cursor, clamping to bounds. Clears pending_wrap.
    pub fn move_cursor_to(&mut self, x: usize, y: usize) {
        self.cursor.x = x.min(self.cols() - 1);
        self.cursor.y = y.min(self.rows() - 1);
        self.cursor.pending_wrap = false;
    }

    /// Write a character at cursor position, advancing cursor.
    pub fn write_char(&mut self, c: char) {
        if self.cursor.pending_wrap {
            // Mark the row we're leaving as soft-wrapped (content continues on next row)
            self.grid.line_wrapped[self.cursor.y] = true;
            self.do_linefeed();
            self.cursor.x = 0;
            self.cursor.pending_wrap = false;
        }

        let col = self.cursor.x;
        let row = self.cursor.y;

        if col < self.cols() && row < self.rows() {
            let cell = self.grid.cell_mut(col, row);
            cell.codepoint = c;
            cell.style = self.cursor.style;
            cell.flags = crate::cell::CellFlags::default();
            if row > self.high_water_row {
                self.high_water_row = row;
            }
        }

        if self.cursor.x >= self.cols() - 1 {
            // At right margin — set pending wrap
            self.cursor.pending_wrap = true;
        } else {
            self.cursor.x += 1;
        }
    }

    /// Line feed: move cursor down, scrolling if needed.
    pub fn do_linefeed(&mut self) {
        if self.cursor.y + 1 >= self.scroll_bottom {
            self.scroll_up(1);
            // Scrolling means content reached the bottom of the scroll region
            let bottom = self.scroll_bottom.saturating_sub(1);
            if bottom > self.high_water_row {
                self.high_water_row = bottom;
            }
        } else {
            self.cursor.y += 1;
            if self.cursor.y > self.high_water_row {
                self.high_water_row = self.cursor.y;
            }
        }
    }

    /// Carriage return: move cursor to column 0.
    pub fn do_carriage_return(&mut self) {
        self.cursor.x = 0;
        self.cursor.pending_wrap = false;
    }

    /// Backspace: move cursor left by 1.
    pub fn do_backspace(&mut self) {
        if self.cursor.x > 0 {
            self.cursor.x -= 1;
            self.cursor.pending_wrap = false;
        }
    }

    /// Horizontal tab: advance to next tab stop.
    pub fn do_tab(&mut self) {
        let x = self.cursor.x + 1;
        for i in x..self.cols() {
            if self.tabstops[i] {
                self.cursor.x = i;
                self.cursor.pending_wrap = false;
                return;
            }
        }
        self.cursor.x = self.cols() - 1;
        self.cursor.pending_wrap = false;
    }

    /// Scroll up within scroll region. Top line goes to scrollback.
    pub fn scroll_up(&mut self, count: usize) {
        // Save scrolled-off lines to scrollback
        if self.max_scrollback > 0 && self.scroll_top == 0 {
            for i in 0..count.min(self.scroll_bottom) {
                let row = self.grid.row_slice(i).to_vec();
                let wrapped = self.grid.line_wrapped[i];
                self.scrollback.push(row);
                self.scrollback_wrapped.push(wrapped);
                if self.scrollback.len() > self.max_scrollback {
                    self.scrollback.remove(0);
                    self.scrollback_wrapped.remove(0);
                }
            }
        }
        self.grid.scroll_up(
            self.scroll_top,
            self.scroll_bottom,
            count,
            self.cursor.style,
        );
    }

    /// Scroll down within scroll region.
    pub fn scroll_down(&mut self, count: usize) {
        self.grid.scroll_down(
            self.scroll_top,
            self.scroll_bottom,
            count,
            self.cursor.style,
        );
    }

    /// Erase in display
    pub fn erase_display(&mut self, mode: u16) {
        let style = self.cursor.style;
        match mode {
            0 => {
                // Below (from cursor to end)
                self.erase_line(0); // cursor to end of line
                self.grid.line_wrapped[self.cursor.y] = false;
                for row in self.cursor.y + 1..self.rows() {
                    self.grid.clear_row(row, style);
                    self.grid.line_wrapped[row] = false;
                }
            }
            1 => {
                // Above (from start to cursor)
                for row in 0..self.cursor.y {
                    self.grid.clear_row(row, style);
                    self.grid.line_wrapped[row] = false;
                }
                self.erase_line(1); // start of line to cursor
            }
            2 => {
                // Entire display
                for row in 0..self.rows() {
                    self.grid.clear_row(row, style);
                    self.grid.line_wrapped[row] = false;
                }
            }
            3 => {
                // Entire display + scrollback
                self.scrollback.clear();
                self.scrollback_wrapped.clear();
                for row in 0..self.rows() {
                    self.grid.clear_row(row, style);
                    self.grid.line_wrapped[row] = false;
                }
            }
            _ => {}
        }
    }

    /// Erase in line
    pub fn erase_line(&mut self, mode: u16) {
        let row = self.cursor.y;
        let style = self.cursor.style;
        match mode {
            0 => {
                // From cursor to end — row can no longer wrap
                for col in self.cursor.x..self.cols() {
                    self.grid.cell_mut(col, row).clear_with_style(style);
                }
                self.grid.line_wrapped[row] = false;
            }
            1 => {
                // From start to cursor
                for col in 0..=self.cursor.x.min(self.cols() - 1) {
                    self.grid.cell_mut(col, row).clear_with_style(style);
                }
            }
            2 => {
                // Entire line
                self.grid.clear_row(row, style);
                self.grid.line_wrapped[row] = false;
            }
            _ => {}
        }
    }

    /// Erase N characters from cursor
    pub fn erase_chars(&mut self, count: usize) {
        let row = self.cursor.y;
        let style = self.cursor.style;
        let end = (self.cursor.x + count).min(self.cols());
        for col in self.cursor.x..end {
            self.grid.cell_mut(col, row).clear_with_style(style);
        }
    }

    /// Insert blank lines at cursor, pushing down
    pub fn insert_lines(&mut self, count: usize) {
        if self.cursor.y >= self.scroll_top && self.cursor.y < self.scroll_bottom {
            self.grid
                .scroll_down(self.cursor.y, self.scroll_bottom, count, self.cursor.style);
        }
    }

    /// Delete lines at cursor, pulling up
    pub fn delete_lines(&mut self, count: usize) {
        if self.cursor.y >= self.scroll_top && self.cursor.y < self.scroll_bottom {
            self.grid
                .scroll_up(self.cursor.y, self.scroll_bottom, count, self.cursor.style);
        }
    }

    /// Insert blank chars at cursor
    pub fn insert_blanks(&mut self, count: usize) {
        self.grid
            .insert_blanks(self.cursor.x, self.cursor.y, count, self.cursor.style);
    }

    /// Delete chars at cursor
    pub fn delete_chars(&mut self, count: usize) {
        self.grid
            .delete_chars(self.cursor.x, self.cursor.y, count, self.cursor.style);
    }

    /// Save cursor position
    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(SavedCursor {
            x: self.cursor.x,
            y: self.cursor.y,
            style: self.cursor.style,
            pending_wrap: self.cursor.pending_wrap,
        });
    }

    /// Restore cursor position
    pub fn restore_cursor(&mut self) {
        if let Some(saved) = &self.saved_cursor {
            self.cursor.x = saved.x.min(self.cols() - 1);
            self.cursor.y = saved.y.min(self.rows() - 1);
            self.cursor.style = saved.style;
            self.cursor.pending_wrap = saved.pending_wrap;
        }
    }

    /// Set scroll region (1-based, inclusive)
    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let top = top.saturating_sub(1); // Convert to 0-based
        let bottom = if bottom == 0 {
            self.rows()
        } else {
            bottom.min(self.rows())
        };
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        }
    }

    /// Resize the screen
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let old_cols = self.cols();
        let old_rows = self.rows();

        if cols == old_cols && rows == old_rows {
            return;
        }

        // Alternate screen (max_scrollback == 0): the TUI app will fully redraw
        // after receiving SIGWINCH, so just create a fresh grid and clamp the
        // cursor. This matches Rio (reflow: false for alt screen) and WezTerm
        // (allow_scrollback == false path).
        if self.max_scrollback == 0 {
            self.grid = Grid::new(cols, rows);
            self.cursor.x = self.cursor.x.min(cols - 1);
            self.cursor.y = self.cursor.y.min(rows - 1);
            self.cursor.pending_wrap = false;
            if let Some(saved) = &mut self.saved_cursor {
                saved.x = saved.x.min(cols - 1);
                saved.y = saved.y.min(rows - 1);
                saved.pending_wrap = false;
            }
            self.scroll_top = 0;
            self.scroll_bottom = rows;
            self.high_water_row = 0;
            self.bottom_trimmed_rows = 0;
            self.tabstops = vec![false; cols];
            for i in (0..cols).step_by(8) {
                self.tabstops[i] = true;
            }
            return;
        }

        // Primary screen.
        if cols != old_cols {
            // Width changed — reflow content at the new width.
            self.reflow_resize(cols, rows);
        } else {
            // Only height changed — manage rows without reflowing.
            self.rows_only_resize(rows);
        }
    }

    /// Resize when only the row count changed (cols stay the same).
    fn rows_only_resize(&mut self, rows: usize) {
        let old_rows = self.rows();
        let cols = self.cols();
        let was_full = self.high_water_row >= old_rows - 1;

        let old_grid_rows: Vec<Vec<Cell>> = (0..old_rows)
            .map(|row| self.grid.row_slice(row).to_vec())
            .collect();
        let old_wrapped: Vec<bool> = self.grid.line_wrapped.clone();
        let mut new_grid = Grid::new(cols, rows);

        let copy_row = |src: &[Cell], dst_row: usize, grid: &mut Grid| {
            for col in 0..cols.min(src.len()) {
                *grid.cell_mut(col, dst_row) = src[col];
            }
        };

        match rows.cmp(&old_rows) {
            std::cmp::Ordering::Greater => {
                // Pull from scrollback when the screen was full, keeping
                // content anchored to the bottom (matches macOS Terminal).
                let pull_count = if was_full {
                    self.scrollback.len().min(rows - old_rows)
                } else {
                    0
                };

                if pull_count > 0 {
                    let sb_start = self.scrollback.len() - pull_count;
                    let pulled_rows: Vec<Vec<Cell>> = self.scrollback.drain(sb_start..).collect();
                    let pulled_wrapped: Vec<bool> =
                        self.scrollback_wrapped.drain(sb_start..).collect();
                    for (i, row) in pulled_rows.iter().enumerate() {
                        copy_row(row, i, &mut new_grid);
                        new_grid.line_wrapped[i] =
                            pulled_wrapped.get(i).copied().unwrap_or(false);
                    }
                    for (src_row, row) in old_grid_rows.iter().enumerate() {
                        copy_row(row, src_row + pull_count, &mut new_grid);
                        new_grid.line_wrapped[src_row + pull_count] = old_wrapped[src_row];
                    }
                    self.cursor.y += pull_count;
                    self.high_water_row += pull_count;
                    self.bottom_trimmed_rows =
                        self.bottom_trimmed_rows.saturating_add(pull_count);
                    if let Some(saved) = &mut self.saved_cursor {
                        saved.y += pull_count;
                    }
                } else {
                    for (src_row, row) in old_grid_rows.iter().enumerate() {
                        copy_row(row, src_row, &mut new_grid);
                        new_grid.line_wrapped[src_row] = old_wrapped[src_row];
                    }
                    if was_full {
                        self.high_water_row = rows - 1;
                    }
                }
            }
            std::cmp::Ordering::Less => {
                let required_scrolling = (self.cursor.y + 1).saturating_sub(rows);
                let lines_removed = old_rows - rows;
                let mut copy_start = required_scrolling.min(lines_removed);
                if copy_start == 0 && self.bottom_trimmed_rows > 0 {
                    copy_start = self.bottom_trimmed_rows.min(lines_removed);
                }

                if copy_start > 0 {
                    for i in 0..copy_start {
                        self.scrollback.push(old_grid_rows[i].clone());
                        self.scrollback_wrapped.push(old_wrapped[i]);
                    }
                    if self.scrollback.len() > self.max_scrollback {
                        let overflow = self.scrollback.len() - self.max_scrollback;
                        self.scrollback.drain(0..overflow);
                        self.scrollback_wrapped.drain(0..overflow);
                    }
                    let consumed = self.bottom_trimmed_rows.min(copy_start);
                    self.bottom_trimmed_rows -= consumed;
                }

                for dst_row in 0..rows {
                    copy_row(&old_grid_rows[dst_row + copy_start], dst_row, &mut new_grid);
                    new_grid.line_wrapped[dst_row] = old_wrapped[dst_row + copy_start];
                }

                self.cursor.y = self.cursor.y.saturating_sub(copy_start).min(rows - 1);
                self.high_water_row =
                    self.high_water_row.saturating_sub(copy_start).min(rows - 1);
                if let Some(saved) = &mut self.saved_cursor {
                    saved.y = saved.y.saturating_sub(copy_start).min(rows - 1);
                }
            }
            std::cmp::Ordering::Equal => unreachable!(),
        }

        self.grid = new_grid;
        self.scroll_top = 0;
        self.scroll_bottom = rows;
    }

    /// Resize with content reflow when the column count changes.
    ///
    /// Only scrollback is reflowed (joined wrapped lines, re-wrapped at new
    /// width).  Grid content is NOT reflowed — it stays at the same row
    /// positions with simple truncate/pad for the width change.  This keeps
    /// TUI coordinate systems stable (apps redraw after SIGWINCH).  Row count
    /// changes are handled identically to `rows_only_resize`.
    fn reflow_resize(&mut self, new_cols: usize, new_rows: usize) {
        let old_cols = self.cols();
        let old_rows = self.rows();
        let was_full = self.high_water_row >= old_rows - 1;

        // --- 1. Reflow scrollback at new width ---
        let mut new_scrollback: Vec<Vec<Cell>> = Vec::new();
        let mut new_scrollback_wrapped: Vec<bool> = Vec::new();
        {
            let mut current: Vec<Cell> = Vec::new();
            for i in 0..self.scrollback.len() {
                let row = &self.scrollback[i];
                let wrapped = self.scrollback_wrapped.get(i).copied().unwrap_or(false);

                if wrapped {
                    // Wrapped row: all columns are content.
                    let take = old_cols.min(row.len());
                    current.extend_from_slice(&row[..take]);
                } else {
                    // Non-wrapped: trim trailing blanks (by codepoint only).
                    let mut end = row.len();
                    while end > 0 && row[end - 1].codepoint == ' ' {
                        end -= 1;
                    }
                    current.extend_from_slice(&row[..end]);

                    // Emit the completed logical line, re-wrapped at new_cols.
                    if current.is_empty() {
                        new_scrollback.push(vec![Cell::default(); new_cols]);
                        new_scrollback_wrapped.push(false);
                    } else {
                        let num_chunks = (current.len() + new_cols - 1) / new_cols;
                        for ci in 0..num_chunks {
                            let begin = ci * new_cols;
                            let end = (begin + new_cols).min(current.len());
                            let mut row = current[begin..end].to_vec();
                            row.resize(new_cols, Cell::default());
                            new_scrollback.push(row);
                            new_scrollback_wrapped.push(ci < num_chunks - 1);
                        }
                    }
                    current.clear();
                }
            }
            // Trailing wrapped content (no terminating non-wrapped row).
            if !current.is_empty() {
                let num_chunks = (current.len() + new_cols - 1) / new_cols;
                for ci in 0..num_chunks {
                    let begin = ci * new_cols;
                    let end = (begin + new_cols).min(current.len());
                    let mut row = current[begin..end].to_vec();
                    row.resize(new_cols, Cell::default());
                    new_scrollback.push(row);
                    new_scrollback_wrapped.push(ci < num_chunks - 1);
                }
            }
        }

        // Trim to max scrollback.
        if new_scrollback.len() > self.max_scrollback {
            let overflow = new_scrollback.len() - self.max_scrollback;
            new_scrollback.drain(0..overflow);
            new_scrollback_wrapped.drain(0..overflow);
        }

        self.scrollback = new_scrollback;
        self.scrollback_wrapped = new_scrollback_wrapped;

        // --- 2. Resize grid (truncate/pad width, same row logic as rows_only_resize) ---
        let old_grid_rows: Vec<Vec<Cell>> = (0..old_rows)
            .map(|row| self.grid.row_slice(row).to_vec())
            .collect();
        let old_wrapped: Vec<bool> = self.grid.line_wrapped.clone();
        let mut new_grid = Grid::new(new_cols, new_rows);

        let copy_row = |src: &[Cell], dst_row: usize, grid: &mut Grid| {
            for col in 0..new_cols.min(src.len()) {
                *grid.cell_mut(col, dst_row) = src[col];
            }
        };

        match new_rows.cmp(&old_rows) {
            std::cmp::Ordering::Greater => {
                let pull_count = if was_full {
                    self.scrollback.len().min(new_rows - old_rows)
                } else {
                    0
                };

                if pull_count > 0 {
                    let sb_start = self.scrollback.len() - pull_count;
                    let pulled_rows: Vec<Vec<Cell>> =
                        self.scrollback.drain(sb_start..).collect();
                    let pulled_wrapped: Vec<bool> =
                        self.scrollback_wrapped.drain(sb_start..).collect();
                    for (i, row) in pulled_rows.iter().enumerate() {
                        copy_row(row, i, &mut new_grid);
                        new_grid.line_wrapped[i] =
                            pulled_wrapped.get(i).copied().unwrap_or(false);
                    }
                    for (src_row, row) in old_grid_rows.iter().enumerate() {
                        copy_row(row, src_row + pull_count, &mut new_grid);
                        new_grid.line_wrapped[src_row + pull_count] = old_wrapped[src_row];
                    }
                    self.cursor.y += pull_count;
                    self.high_water_row += pull_count;
                    self.bottom_trimmed_rows =
                        self.bottom_trimmed_rows.saturating_add(pull_count);
                    if let Some(saved) = &mut self.saved_cursor {
                        saved.y += pull_count;
                    }
                } else {
                    for (src_row, row) in old_grid_rows.iter().enumerate() {
                        copy_row(row, src_row, &mut new_grid);
                        new_grid.line_wrapped[src_row] = old_wrapped[src_row];
                    }
                    if was_full {
                        self.high_water_row = new_rows - 1;
                    }
                }
            }
            std::cmp::Ordering::Less => {
                let required_scrolling = (self.cursor.y + 1).saturating_sub(new_rows);
                let lines_removed = old_rows - new_rows;
                let mut copy_start = required_scrolling.min(lines_removed);
                if copy_start == 0 && self.bottom_trimmed_rows > 0 {
                    copy_start = self.bottom_trimmed_rows.min(lines_removed);
                }

                if copy_start > 0 {
                    for i in 0..copy_start {
                        let mut row = old_grid_rows[i].clone();
                        row.resize(new_cols, Cell::default());
                        row.truncate(new_cols);
                        self.scrollback.push(row);
                        self.scrollback_wrapped.push(old_wrapped[i]);
                    }
                    if self.scrollback.len() > self.max_scrollback {
                        let overflow = self.scrollback.len() - self.max_scrollback;
                        self.scrollback.drain(0..overflow);
                        self.scrollback_wrapped.drain(0..overflow);
                    }
                    let consumed = self.bottom_trimmed_rows.min(copy_start);
                    self.bottom_trimmed_rows -= consumed;
                }

                for dst_row in 0..new_rows {
                    copy_row(
                        &old_grid_rows[dst_row + copy_start],
                        dst_row,
                        &mut new_grid,
                    );
                    new_grid.line_wrapped[dst_row] = old_wrapped[dst_row + copy_start];
                }

                self.cursor.y = self.cursor.y.saturating_sub(copy_start).min(new_rows - 1);
                self.high_water_row =
                    self.high_water_row.saturating_sub(copy_start).min(new_rows - 1);
                if let Some(saved) = &mut self.saved_cursor {
                    saved.y = saved.y.saturating_sub(copy_start).min(new_rows - 1);
                }
            }
            std::cmp::Ordering::Equal => {
                // Only cols changed — copy rows with truncate/pad.
                for (src_row, row) in old_grid_rows.iter().enumerate() {
                    copy_row(row, src_row, &mut new_grid);
                    new_grid.line_wrapped[src_row] = old_wrapped[src_row];
                }
            }
        }

        self.grid = new_grid;
        self.cursor.x = self.cursor.x.min(new_cols - 1);
        self.cursor.pending_wrap = false;
        self.scroll_top = 0;
        self.scroll_bottom = new_rows;

        if let Some(saved) = &mut self.saved_cursor {
            saved.x = saved.x.min(new_cols - 1);
            saved.pending_wrap = false;
        }

        self.tabstops = vec![false; new_cols];
        for i in (0..new_cols).step_by(8) {
            self.tabstops[i] = true;
        }
    }

    /// Get scrollback lines
    pub fn scrollback(&self) -> &[Vec<Cell>] {
        &self.scrollback
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Number of grid rows that have been written to (based on high water mark).
    pub fn used_rows(&self) -> usize {
        (self.high_water_row + 1).min(self.rows())
    }

    /// Total logical rows visible through a virtual viewport
    /// (`scrollback` + active grid rows).
    pub fn total_rows(&self) -> usize {
        self.scrollback.len() + self.rows()
    }

    /// Row slice by virtual row index where:
    /// - `0..scrollback_len` maps to historical rows (oldest first)
    /// - `scrollback_len..total_rows` maps to active grid rows.
    pub fn row_slice_virtual(&self, row: usize) -> Option<&[Cell]> {
        if row < self.scrollback.len() {
            return Some(&self.scrollback[row]);
        }
        let grid_row = row - self.scrollback.len();
        if grid_row < self.rows() {
            return Some(self.grid.row_slice(grid_row));
        }
        None
    }
}
