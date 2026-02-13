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
    pub max_scrollback: usize,

    // Tab stops
    pub tabstops: Vec<bool>,
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
            max_scrollback: if with_scrollback { 10000 } else { 0 },
            tabstops,
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
        } else {
            self.cursor.y += 1;
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
                self.scrollback.push(row);
                if self.scrollback.len() > self.max_scrollback {
                    self.scrollback.remove(0);
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
                for row in self.cursor.y + 1..self.rows() {
                    self.grid.clear_row(row, style);
                }
            }
            1 => {
                // Above (from start to cursor)
                for row in 0..self.cursor.y {
                    self.grid.clear_row(row, style);
                }
                self.erase_line(1); // start of line to cursor
            }
            2 => {
                // Entire display
                for row in 0..self.rows() {
                    self.grid.clear_row(row, style);
                }
            }
            3 => {
                // Entire display + scrollback
                self.scrollback.clear();
                for row in 0..self.rows() {
                    self.grid.clear_row(row, style);
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
                // From cursor to end
                for col in self.cursor.x..self.cols() {
                    self.grid.cell_mut(col, row).clear_with_style(style);
                }
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
        self.grid.resize(cols, rows);
        self.scroll_top = 0;
        self.scroll_bottom = rows;
        self.cursor.x = self.cursor.x.min(cols.saturating_sub(1));
        self.cursor.y = self.cursor.y.min(rows.saturating_sub(1));

        // Reset tabstops
        self.tabstops = vec![false; cols];
        for i in (0..cols).step_by(8) {
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
}
